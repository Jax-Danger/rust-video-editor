//! CPU composite for the program monitor.
//!
//! Decoded frames stay in the preview cache. This pass applies the same grade
//! formula as the proxy cards, then scale, position, rotation, opacity, and a
//! horizontal wipe mask. It runs at preview resolution, not at sequence size.

use editor_core::{color_grade, ColorGrade, Transform};

#[derive(Clone, Copy, Debug)]
pub struct GradeSample {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub temperature: f32,
    pub tint: f32,
    pub saturation: f32,
}

impl GradeSample {
    pub fn neutral() -> Self {
        Self {
            exposure: 0.0,
            contrast: 1.0,
            highlights: 0.0,
            shadows: 0.0,
            temperature: 0.0,
            tint: 0.0,
            saturation: 1.0,
        }
    }

    pub fn from_grade(grade: &ColorGrade, rel: i64) -> Self {
        Self {
            exposure: grade.exposure.value_at(rel),
            contrast: grade.contrast.value_at(rel),
            highlights: grade.highlights.value_at(rel),
            shadows: grade.shadows.value_at(rel),
            temperature: grade.temperature.value_at(rel),
            tint: grade.tint.value_at(rel),
            saturation: grade.saturation.value_at(rel),
        }
    }

    pub fn from_effects(effects: &[editor_core::Effect], rel: i64) -> Self {
        color_grade(effects)
            .map(|grade| Self::from_grade(grade, rel))
            .unwrap_or_else(Self::neutral)
    }

    #[allow(dead_code)]
    pub fn is_neutral(self) -> bool {
        self.exposure.abs() < 1.0e-4
            && (self.contrast - 1.0).abs() < 1.0e-4
            && self.highlights.abs() < 1.0e-4
            && self.shadows.abs() < 1.0e-4
            && self.temperature.abs() < 1.0e-4
            && self.tint.abs() < 1.0e-4
            && (self.saturation - 1.0).abs() < 1.0e-4
    }

    pub fn apply(self, rgb: [f32; 3]) -> [f32; 3] {
        let mut c = rgb.map(|channel| channel * 2.0_f32.powf(self.exposure));
        c = c.map(|channel| ((channel - 0.5) * self.contrast + 0.5).clamp(0.0, 1.5));
        c = c.map(|channel| {
            let shadow_w = (1.0 - channel).clamp(0.0, 1.0);
            let high_w = channel.clamp(0.0, 1.0);
            (channel + self.shadows * 0.35 * shadow_w + self.highlights * 0.35 * high_w)
                .clamp(0.0, 1.5)
        });
        c[0] = (c[0] + self.temperature * 0.18).clamp(0.0, 1.5);
        c[2] = (c[2] - self.temperature * 0.18).clamp(0.0, 1.5);
        c[1] = (c[1] - self.tint * 0.14).clamp(0.0, 1.5);
        c[0] = (c[0] + self.tint * 0.06).clamp(0.0, 1.5);
        c[2] = (c[2] + self.tint * 0.06).clamp(0.0, 1.5);
        let luma = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        c.map(|channel| (luma + (channel - luma) * self.saturation).clamp(0.0, 1.0))
    }
}

pub fn place_from_transform(xform: &Transform, rel: i64, mix: f32) -> Place {
    Place {
        scale_x: xform.scale_x.value_at(rel).max(0.01),
        scale_y: xform.scale_y.value_at(rel).max(0.01),
        pos_x: xform.position_x.value_at(rel),
        pos_y: xform.position_y.value_at(rel),
        rotation: xform.rotation_deg.value_at(rel),
        anchor_x: xform.anchor_x.value_at(rel),
        anchor_y: xform.anchor_y.value_at(rel),
        opacity: (xform.opacity.value_at(rel) * mix).clamp(0.0, 1.0),
        clip_u0: 0.0,
        clip_u1: 1.0,
        shift_x: 0.0,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Place {
    pub scale_x: f32,
    pub scale_y: f32,
    pub pos_x: f32,
    pub pos_y: f32,
    pub rotation: f32,
    pub anchor_x: f32,
    pub anchor_y: f32,
    pub opacity: f32,
    /// Visible horizontal window of the canvas, 0…1. A wipe uses this.
    pub clip_u0: f32,
    pub clip_u1: f32,
    /// Extra sequence-pixel slide, used by push transitions.
    pub shift_x: f32,
}

pub struct BlitLayer<'a> {
    pub rgba: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub grade: GradeSample,
    pub place: Place,
}

pub struct PictureCache {
    pub signature: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn composite(dst_w: u32, dst_h: u32, seq_w: f32, seq_h: f32, layers: &[BlitLayer<'_>]) -> Vec<u8> {
    let mut dst = vec![0u8; dst_w as usize * dst_h as usize * 4];
    if dst_w == 0 || dst_h == 0 || seq_w <= 1.0 || seq_h <= 1.0 {
        return dst;
    }
    for layer in layers {
        if layer.place.opacity <= 0.001 || layer.rgba.len() < 4 {
            continue;
        }
        if straight_full_frame(layer, dst_w, dst_h) {
            grade_over(&mut dst, dst_w, layer);
        } else {
            blit(&mut dst, dst_w, dst_h, seq_w, seq_h, layer);
        }
    }
    dst
}

fn straight_full_frame(layer: &BlitLayer<'_>, dst_w: u32, dst_h: u32) -> bool {
    let place = layer.place;
    layer.width == dst_w
        && layer.height == dst_h
        && (place.scale_x - 1.0).abs() < 0.01
        && (place.scale_y - 1.0).abs() < 0.01
        && place.pos_x.abs() < 0.5
        && place.pos_y.abs() < 0.5
        && place.shift_x.abs() < 0.5
        && place.rotation.abs() < 0.05
        && (place.anchor_x - 0.5).abs() < 0.01
        && (place.anchor_y - 0.5).abs() < 0.01
        && place.clip_u0 <= 0.001
        && place.clip_u1 >= 0.999
}

fn grade_over(dst: &mut [u8], dst_w: u32, layer: &BlitLayer<'_>) {
    let pixels = (dst_w as usize) * (layer.height as usize);
    let count = pixels.min(layer.rgba.len() / 4).min(dst.len() / 4);
    for index in 0..count {
        let src = &layer.rgba[index * 4..index * 4 + 4];
        let rgb = layer.grade.apply([
            src[0] as f32 / 255.0,
            src[1] as f32 / 255.0,
            src[2] as f32 / 255.0,
        ]);
        let alpha = src[3] as f32 / 255.0 * layer.place.opacity;
        over(&mut dst[index * 4..index * 4 + 4], rgb, alpha);
    }
}

fn blit(dst: &mut [u8], dst_w: u32, dst_h: u32, seq_w: f32, seq_h: f32, layer: &BlitLayer<'_>) {
    let place = layer.place;
    let sx = dst_w as f32 / seq_w;
    let sy = dst_h as f32 / seq_h;
    let disp_w = (dst_w as f32 * place.scale_x).max(1.0);
    let disp_h = (dst_h as f32 * place.scale_y).max(1.0);
    let center_x = dst_w as f32 * 0.5 + (place.pos_x + place.shift_x) * sx;
    let center_y = dst_h as f32 * 0.5 - place.pos_y * sy;
    let local_anchor_x = (place.anchor_x - 0.5) * disp_w;
    let local_anchor_y = (0.5 - place.anchor_y) * disp_h;
    let rotated_anchor = rotate(local_anchor_x, local_anchor_y, place.rotation);
    let origin_x = center_x - rotated_anchor.0;
    let origin_y = center_y - rotated_anchor.1;
    let x0 = ((place.clip_u0.clamp(0.0, 1.0) * dst_w as f32).floor() as i32).max(0);
    let x1 = ((place.clip_u1.clamp(0.0, 1.0) * dst_w as f32).ceil() as i32).min(dst_w as i32);
    let radius = (disp_w.hypot(disp_h) * 0.5 + 2.0) as i32;
    let min_y = (origin_y as i32 - radius).clamp(0, dst_h as i32);
    let max_y = (origin_y as i32 + radius + 1).clamp(0, dst_h as i32);
    let min_x = (origin_x as i32 - radius).clamp(x0, x1);
    let max_x = (origin_x as i32 + radius + 1).clamp(x0, x1);
    let src_w = layer.width as i32;
    let src_h = layer.height as i32;
    for y in min_y..max_y {
        for x in min_x..max_x {
            let (local_x, local_y) = rotate(
                x as f32 + 0.5 - origin_x,
                y as f32 + 0.5 - origin_y,
                -place.rotation,
            );
            if local_x.abs() > disp_w * 0.5 || local_y.abs() > disp_h * 0.5 {
                continue;
            }
            let u = local_x / disp_w + 0.5;
            let v = local_y / disp_h + 0.5;
            let sample = sample(layer.rgba, src_w, src_h, u * src_w as f32 - 0.5, v * src_h as f32 - 0.5);
            let rgb = layer.grade.apply([sample[0], sample[1], sample[2]]);
            let alpha = sample[3] * place.opacity;
            let index = (y as usize * dst_w as usize + x as usize) * 4;
            over(&mut dst[index..index + 4], rgb, alpha);
        }
    }
}

fn sample(src: &[u8], width: i32, height: i32, x: f32, y: f32) -> [f32; 4] {
    if width <= 0 || height <= 0 {
        return [0.0; 4];
    }
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let p00 = pixel(src, width, height, x0, y0);
    let p10 = pixel(src, width, height, x0 + 1, y0);
    let p01 = pixel(src, width, height, x0, y0 + 1);
    let p11 = pixel(src, width, height, x0 + 1, y0 + 1);
    let mut out = [0.0; 4];
    for channel in 0..4 {
        let top = p00[channel] + (p10[channel] - p00[channel]) * tx;
        let bottom = p01[channel] + (p11[channel] - p01[channel]) * tx;
        out[channel] = top + (bottom - top) * ty;
    }
    out
}

fn pixel(src: &[u8], width: i32, height: i32, x: i32, y: i32) -> [f32; 4] {
    if x < 0 || y < 0 || x >= width || y >= height {
        return [0.0; 4];
    }
    let index = (y as usize * width as usize + x as usize) * 4;
    if index + 3 >= src.len() {
        return [0.0; 4];
    }
    [
        src[index] as f32 / 255.0,
        src[index + 1] as f32 / 255.0,
        src[index + 2] as f32 / 255.0,
        src[index + 3] as f32 / 255.0,
    ]
}

fn over(dst: &mut [u8], rgb: [f32; 3], src_a: f32) {
    if src_a <= 0.001 {
        return;
    }
    let src_a = src_a.clamp(0.0, 1.0);
    let dst_a = dst[3] as f32 / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= 1.0e-4 {
        return;
    }
    for channel in 0..3 {
        let src = rgb[channel];
        let old = dst[channel] as f32 / 255.0;
        let mixed = (src * src_a + old * dst_a * (1.0 - src_a)) / out_a;
        dst[channel] = (mixed.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    dst[3] = (out_a.clamp(0.0, 1.0) * 255.0).round() as u8;
}

fn rotate(x: f32, y: f32, degrees: f32) -> (f32, f32) {
    let (sin, cos) = degrees.to_radians().sin_cos();
    (x * cos - y * sin, x * sin + y * cos)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            pixels.extend_from_slice(&rgba);
        }
        pixels
    }

    #[test]
    fn exposure_lifts_a_mid_grey() {
        let grade = GradeSample {
            exposure: 1.0,
            ..GradeSample::neutral()
        };
        let lifted = grade.apply([0.5, 0.5, 0.5]);
        assert!(lifted[0] > 0.9, "{lifted:?}");
        let neutral = GradeSample::neutral().apply([0.25, 0.5, 0.75]);
        assert!((neutral[1] - 0.5).abs() < 1.0e-4);
    }

    #[test]
    fn full_frame_grade_and_opacity_composite() {
        let src = solid(2, 2, [128, 128, 128, 255]);
        let layers = [BlitLayer {
            rgba: &src,
            width: 2,
            height: 2,
            grade: GradeSample {
                exposure: 1.0,
                ..GradeSample::neutral()
            },
            place: Place {
                scale_x: 1.0,
                scale_y: 1.0,
                pos_x: 0.0,
                pos_y: 0.0,
                rotation: 0.0,
                anchor_x: 0.5,
                anchor_y: 0.5,
                opacity: 0.5,
                clip_u0: 0.0,
                clip_u1: 1.0,
                shift_x: 0.0,
            },
        }];
        let out = composite(2, 2, 2.0, 2.0, &layers);
        assert!(out[0] > 180, "{}", out[0]);
        assert!((out[3] as i32 - 128).abs() < 2, "{}", out[3]);
    }

    #[test]
    fn wipe_keeps_only_the_right_half() {
        let src = solid(4, 2, [0, 200, 0, 255]);
        let layers = [BlitLayer {
            rgba: &src,
            width: 4,
            height: 2,
            grade: GradeSample::neutral(),
            place: Place {
                scale_x: 1.0,
                scale_y: 1.0,
                pos_x: 0.0,
                pos_y: 0.0,
                rotation: 0.0,
                anchor_x: 0.5,
                anchor_y: 0.5,
                opacity: 1.0,
                clip_u0: 0.5,
                clip_u1: 1.0,
                shift_x: 0.0,
            },
        }];
        let out = composite(4, 2, 4.0, 2.0, &layers);
        assert_eq!(out[3], 0, "left pixel should be clear");
        assert_eq!(out[(2 * 4 + 3) as usize], 255);
    }
}
