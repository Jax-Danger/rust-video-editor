//! Clip stabilization: block-match motion estimation and temporal smoothing.
//!
//! Analysis can be stored on the clip effect (`keyframes`) or in a sidecar JSON
//! file next to the media (`<path>.stabilize.json`). At compose time, when no
//! baked keyframes exist, a lightweight runtime pass estimates motion from
//! consecutive decoded frames and applies a causal low-pass correction.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// Per-source-frame motion sample (cumulative offset from frame 0).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MotionSample {
    pub frame: i64,
    pub dx: f32,
    pub dy: f32,
    #[serde(default)]
    pub rotation_deg: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StabilizeSidecar {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub samples: Vec<MotionSample>,
}

/// Block-match displacement between two RGBA frames (sequence pixels).
pub fn block_match_offset(
    prev: &[u8],
    curr: &[u8],
    width: u32,
    height: u32,
    search: i32,
) -> (f32, f32) {
    if prev.len() != curr.len() || width < 8 || height < 8 {
        return (0.0, 0.0);
    }
    let coarse = centroid_delta(prev, curr, width, height);
    let mut best = (
        coarse.0.round().clamp(-search as f32, search as f32) as i32,
        coarse.1.round().clamp(-search as f32, search as f32) as i32,
    );
    let mut best_cost = i64::MAX;
    let block = 16u32.min(width / 3).max(8);
    let bx = (width / 2).saturating_sub(block / 2);
    let by = (height / 2).saturating_sub(block / 2);
    let refine = 4i32.min(search);
    for dy in (best.1 - refine)..=(best.1 + refine) {
        for dx in (best.0 - refine)..=(best.0 + refine) {
            if dx.abs() > search || dy.abs() > search {
                continue;
            }
            let cost = block_sad(prev, curr, width, height, bx, by, block, dx, dy);
            if cost < best_cost {
                best_cost = cost;
                best = (dx, dy);
            }
        }
    }
    (best.0 as f32, best.1 as f32)
}

fn centroid_delta(prev: &[u8], curr: &[u8], width: u32, height: u32) -> (f32, f32) {
    let (px, py) = feature_centroid(prev, width, height);
    let (cx, cy) = feature_centroid(curr, width, height);
    (cx - px, cy - py)
}

pub fn feature_centroid(rgba: &[u8], width: u32, height: u32) -> (f32, f32) {
    let mut sx = 0.0f64;
    let mut sy = 0.0f64;
    let mut weight = 0.0f64;
    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            if rgba[idx + 3] == 0 {
                continue;
            }
            let luma = 0.2126 * rgba[idx] as f32
                + 0.7152 * rgba[idx + 1] as f32
                + 0.0722 * rgba[idx + 2] as f32;
            let w = (luma / 255.0).powi(2) as f64;
            if w <= 1.0e-4 {
                continue;
            }
            sx += x as f64 * w;
            sy += y as f64 * w;
            weight += w;
        }
    }
    if weight <= 1.0e-6 {
        return (width as f32 * 0.5, height as f32 * 0.5);
    }
    ((sx / weight) as f32, (sy / weight) as f32)
}

fn block_sad(
    prev: &[u8],
    curr: &[u8],
    width: u32,
    height: u32,
    bx: u32,
    by: u32,
    block: u32,
    dx: i32,
    dy: i32,
) -> i64 {
    let mut sum = 0i64;
    for y in 0..block {
        let sy = by + y;
        let ty = sy as i32 + dy;
        if sy >= height || ty < 0 || ty >= height as i32 {
            continue;
        }
        for x in 0..block {
            let sx = bx + x;
            let tx = sx as i32 + dx;
            if sx >= width || tx < 0 || tx >= width as i32 {
                continue;
            }
            let pi = ((sy * width + sx) * 4) as usize;
            let ti = ((ty as u32 * width + tx as u32) * 4) as usize;
            if pi + 3 >= prev.len() || ti + 3 >= curr.len() {
                continue;
            }
            for c in 0..3 {
                let d = prev[pi + c] as i32 - curr[ti + c] as i32;
                sum += (d * d) as i64;
            }
        }
    }
    sum
}

/// Integrate per-frame deltas into cumulative motion, then low-pass the path.
pub fn analyze_motion_path(frames: &[&[u8]], width: u32, height: u32, search: i32) -> Vec<MotionSample> {
    if frames.is_empty() {
        return Vec::new();
    }
    let mut cumulative = (0.0f32, 0.0f32);
    let mut out = Vec::with_capacity(frames.len());
    out.push(MotionSample {
        frame: 0,
        dx: 0.0,
        dy: 0.0,
        rotation_deg: 0.0,
    });
    for index in 1..frames.len() {
        let (dx, dy) = block_match_offset(frames[index - 1], frames[index], width, height, search);
        cumulative.0 += dx;
        cumulative.1 += dy;
        out.push(MotionSample {
            frame: index as i64,
            dx: cumulative.0,
            dy: cumulative.1,
            rotation_deg: 0.0,
        });
    }
    out
}

/// Smoothing 0…1 maps to a centered moving-average window (3…31 frames).
pub fn smooth_motion(samples: &[MotionSample], smoothing: f32) -> Vec<MotionSample> {
    if samples.is_empty() {
        return Vec::new();
    }
    let window = smoothing_window(smoothing);
    let half = window / 2;
    samples
        .iter()
        .enumerate()
        .map(|(index, sample)| {
            let start = index.saturating_sub(half);
            let end = (index + half + 1).min(samples.len());
            let count = (end - start).max(1) as f32;
            let mut dx = 0.0f32;
            let mut dy = 0.0f32;
            let mut rot = 0.0f32;
            for item in &samples[start..end] {
                dx += item.dx;
                dy += item.dy;
                rot += item.rotation_deg;
            }
            MotionSample {
                frame: sample.frame,
                dx: dx / count,
                dy: dy / count,
                rotation_deg: rot / count,
            }
        })
        .collect()
}

fn smoothing_window(smoothing: f32) -> usize {
    let t = smoothing.clamp(0.05, 1.0);
    (3.0 + t * 28.0).round() as usize | 1
}

/// Correction that steadies the picture: smoothed path minus raw cumulative motion.
pub fn correction_at(
    raw: &[MotionSample],
    smoothed: &[MotionSample],
    frame: i64,
    strength: f32,
) -> (f32, f32, f32) {
    let strength = strength.clamp(0.0, 1.0);
    if strength <= 1.0e-4 || raw.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let index = raw
        .iter()
        .position(|sample| sample.frame == frame)
        .unwrap_or_else(|| frame.clamp(0, raw.len().saturating_sub(1) as i64) as usize);
    let raw_sample = &raw[index.min(raw.len() - 1)];
    let smooth_sample = &smoothed[index.min(smoothed.len() - 1)];
    let dx = (smooth_sample.dx - raw_sample.dx) * strength;
    let dy = (smooth_sample.dy - raw_sample.dy) * strength;
    let rot = (smooth_sample.rotation_deg - raw_sample.rotation_deg) * strength;
    (dx, dy, rot)
}

pub fn sidecar_path(media_path: &str) -> PathBuf {
    Path::new(media_path).with_extension("stabilize.json")
}

pub fn load_sidecar(media_path: &str) -> Option<StabilizeSidecar> {
    let path = sidecar_path(media_path);
    let text = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_sidecar(sidecar: &StabilizeSidecar) -> Result<(), String> {
    let path = sidecar_path(&sidecar.path);
    let json = serde_json::to_string_pretty(sidecar).map_err(|err| err.to_string())?;
    std::fs::write(&path, json).map_err(|err| err.to_string())?;
    Ok(())
}

/// Runtime tracker for causal stabilization during sequential compose/export.
#[derive(Default)]
pub struct StabilizeRuntime {
    tracks: HashMap<u64, TrackState>,
}

struct TrackState {
    width: u32,
    height: u32,
    raw: Vec<MotionSample>,
    prev_rgba: Option<Vec<u8>>,
}

impl StabilizeRuntime {
    pub fn reset(&mut self) {
        self.tracks.clear();
    }

    pub fn correction_for_frame(
        &mut self,
        clip_id: u64,
        source_frame: i64,
        rgba: &[u8],
        width: u32,
        height: u32,
        strength: f32,
        smoothing: f32,
    ) -> (f32, f32, f32) {
        if strength <= 1.0e-4 {
            return (0.0, 0.0, 0.0);
        }
        let track = self.tracks.entry(clip_id).or_insert_with(|| TrackState {
            width,
            height,
            raw: vec![MotionSample {
                frame: source_frame,
                dx: 0.0,
                dy: 0.0,
                rotation_deg: 0.0,
            }],
            prev_rgba: None,
        });
        track.width = width;
        track.height = height;

        if let Some(prev) = &track.prev_rgba {
            if prev.len() == rgba.len() {
                let (dx, dy) = block_match_offset(prev, rgba, width, height, 12);
                let last = track.raw.last().cloned().unwrap_or(MotionSample {
                    frame: source_frame,
                    dx: 0.0,
                    dy: 0.0,
                    rotation_deg: 0.0,
                });
                if source_frame > last.frame {
                    track.raw.push(MotionSample {
                        frame: source_frame,
                        dx: last.dx + dx,
                        dy: last.dy + dy,
                        rotation_deg: 0.0,
                    });
                }
            }
        } else if track.raw.is_empty() {
            track.raw.push(MotionSample {
                frame: source_frame,
                dx: 0.0,
                dy: 0.0,
                rotation_deg: 0.0,
            });
        }
        track.prev_rgba = Some(rgba.to_vec());

        let smoothed = smooth_motion(&track.raw, smoothing);
        correction_at(&track.raw, &smoothed, source_frame, strength)
    }
}

static RUNTIME: OnceLock<Mutex<StabilizeRuntime>> = OnceLock::new();

pub fn stabilize_runtime() -> &'static Mutex<StabilizeRuntime> {
    RUNTIME.get_or_init(|| Mutex::new(StabilizeRuntime::default()))
}

pub fn baked_correction(
    keyframes: &[MotionSample],
    frame: i64,
    strength: f32,
    smoothing: f32,
) -> (f32, f32, f32) {
    if keyframes.is_empty() || strength <= 1.0e-4 {
        return (0.0, 0.0, 0.0);
    }
    let smoothed = smooth_motion(keyframes, smoothing);
    correction_at(keyframes, &smoothed, frame, strength)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        for px in rgba.chunks_exact_mut(4) {
            px[0] = rgb[0];
            px[1] = rgb[1];
            px[2] = rgb[2];
            px[3] = 255;
        }
        rgba
    }

    fn shift_rgba(src: &[u8], width: u32, height: u32, dx: i32, dy: i32) -> Vec<u8> {
        let mut out = vec![0u8; src.len()];
        for y in 0..height {
            for x in 0..width {
                let sx = x as i32 - dx;
                let sy = y as i32 - dy;
                let dst = ((y * width + x) * 4) as usize;
                if sx < 0 || sy < 0 || sx >= width as i32 || sy >= height as i32 {
                    continue;
                }
                let src_idx = ((sy as u32 * width + sx as u32) * 4) as usize;
                out[dst..dst + 4].copy_from_slice(&src[src_idx..src_idx + 4]);
            }
        }
        out
    }

    fn centroid_x(rgba: &[u8], width: u32, height: u32) -> f32 {
        let mut sum = 0.0f64;
        let mut weight = 0.0f64;
        for y in 0..height {
            for x in 0..width {
                let idx = ((y * width + x) * 4) as usize;
                if rgba[idx + 3] > 0 {
                    sum += x as f64;
                    weight += 1.0;
                }
            }
        }
        if weight <= 0.0 {
            0.0
        } else {
            (sum / weight) as f32
        }
    }

    fn stamp_dot(rgba: &mut [u8], width: u32, height: u32, cx: u32, cy: u32) {
        for y in cy.saturating_sub(4)..cy.saturating_add(5).min(height) {
            for x in cx.saturating_sub(4)..cx.saturating_add(5).min(width) {
                let idx = ((y * width + x) * 4) as usize;
                rgba[idx] = 255;
                rgba[idx + 1] = 240;
                rgba[idx + 2] = 40;
            }
        }
    }

    #[test]
    fn block_match_finds_horizontal_shift() {
        let width = 64u32;
        let height = 48u32;
        let mut base = solid(width, height, [20, 30, 40]);
        stamp_dot(&mut base, width, height, 32, 24);
        let shifted = shift_rgba(&base, width, height, 5, -3);
        let (dx, dy) = block_match_offset(&base, &shifted, width, height, 8);
        assert!((dx - 5.0).abs() < 1.0, "dx={dx}");
        assert!((dy + 3.0).abs() < 1.0, "dy={dy}");
    }

    #[test]
    fn synthetic_shake_variance_drops_after_stabilize() {
        let width = 64u32;
        let height = 48u32;
        let mut base = solid(width, height, [20, 30, 40]);
        stamp_dot(&mut base, width, height, 32, 24);
        let mut frames = Vec::new();
        let mut raw_shake = Vec::new();
        for index in 0..24 {
            let dx = (index as f32 * 0.9).sin() * 8.0;
            let dy = (index as f32 * 0.7).cos() * 6.0;
            raw_shake.push((dx.round() as i32, dy.round() as i32));
            frames.push(shift_rgba(&base, width, height, dx.round() as i32, dy.round() as i32));
        }
        let refs: Vec<&[u8]> = frames.iter().map(|f| f.as_slice()).collect();
        let motion = analyze_motion_path(&refs, width, height, 12);
        let smoothed = smooth_motion(&motion, 0.65);
        let strength = 1.0;
        let mut unstable = Vec::new();
        let mut stable = Vec::new();
        for (index, frame) in frames.iter().enumerate() {
            let source_frame = index as i64;
            let (cx, cy, _) = correction_at(&motion, &smoothed, source_frame, strength);
            let corrected = shift_rgba(frame, width, height, cx.round() as i32, cy.round() as i32);
            unstable.push(centroid_x(frame, width, height));
            stable.push(centroid_x(&corrected, width, height));
        }
        let var = |values: &[f32]| {
            let mean = values.iter().sum::<f32>() / values.len().max(1) as f32;
            values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / values.len().max(1) as f32
        };
        assert!(
            var(&stable) < var(&unstable),
            "stable var {} should be lower than unstable {}",
            var(&stable),
            var(&unstable)
        );
    }

    #[test]
    fn runtime_track_builds_causal_correction() {
        let width = 48u32;
        let height = 36u32;
        let mut base = solid(width, height, [20, 30, 40]);
        stamp_dot(&mut base, width, height, 24, 18);
        let mut runtime = StabilizeRuntime::default();
        let mut last_corr = (0.0, 0.0, 0.0);
        for index in 0..12 {
            let dx = (index as f32).sin() * 5.0;
            let dy = (index as f32 * 1.3).cos() * 4.0;
            let frame = shift_rgba(&base, width, height, dx.round() as i32, dy.round() as i32);
            last_corr = runtime.correction_for_frame(1, index as i64, &frame, width, height, 1.0, 0.6);
        }
        assert!(last_corr.0.abs() + last_corr.1.abs() > 0.5);
    }

    #[test]
    fn sidecar_round_trip() {
        let sidecar = StabilizeSidecar {
            path: "/tmp/shake.mp4".into(),
            width: 64,
            height: 48,
            samples: vec![
                MotionSample {
                    frame: 0,
                    dx: 0.0,
                    dy: 0.0,
                    rotation_deg: 0.0,
                },
                MotionSample {
                    frame: 1,
                    dx: 2.0,
                    dy: -1.0,
                    rotation_deg: 0.0,
                },
            ],
        };
        let json = serde_json::to_string(&sidecar).unwrap();
        let parsed: StabilizeSidecar = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, sidecar);
    }
}
