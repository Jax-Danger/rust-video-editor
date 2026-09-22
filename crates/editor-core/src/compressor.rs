//! Per-track dynamics compressor shared by playback and export.
//!
//! Stereo-linked peak detector with programmable threshold, ratio, attack,
//! release, and makeup gain. Applied after EQ and before the track fader.

use serde::{Deserialize, Serialize};

use crate::mix::db_to_linear;

pub const COMP_SAMPLE_RATE: f32 = 48_000.0;

pub const THRESHOLD_DB_MIN: f32 = -60.0;
pub const THRESHOLD_DB_MAX: f32 = 0.0;
pub const RATIO_MIN: f32 = 1.0;
pub const RATIO_MAX: f32 = 20.0;
pub const ATTACK_MS_MIN: f32 = 0.1;
pub const ATTACK_MS_MAX: f32 = 100.0;
pub const RELEASE_MS_MIN: f32 = 10.0;
pub const RELEASE_MS_MAX: f32 = 1_000.0;
pub const MAKEUP_DB_MIN: f32 = 0.0;
pub const MAKEUP_DB_MAX: f32 = 24.0;

pub const DEFAULT_ATTACK_MS: f32 = 10.0;
pub const DEFAULT_RELEASE_MS: f32 = 100.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackCompressor {
    /// Level in dBFS above which gain reduction begins.
    #[serde(default, skip_serializing_if = "is_default_threshold")]
    pub threshold_db: f32,
    /// Compression ratio above the threshold. 1:1 is bypass.
    #[serde(default = "default_ratio", skip_serializing_if = "is_bypass_ratio")]
    pub ratio: f32,
    /// Attack time in milliseconds.
    #[serde(
        default = "default_attack_ms",
        skip_serializing_if = "is_default_attack"
    )]
    pub attack_ms: f32,
    /// Release time in milliseconds.
    #[serde(
        default = "default_release_ms",
        skip_serializing_if = "is_default_release"
    )]
    pub release_ms: f32,
    /// Output makeup gain in dB.
    #[serde(default, skip_serializing_if = "is_zero_db")]
    pub makeup_db: f32,
}

impl Default for TrackCompressor {
    fn default() -> Self {
        Self {
            threshold_db: 0.0,
            ratio: 1.0,
            attack_ms: DEFAULT_ATTACK_MS,
            release_ms: DEFAULT_RELEASE_MS,
            makeup_db: 0.0,
        }
    }
}

impl TrackCompressor {
    pub fn is_bypass(&self) -> bool {
        self.ratio <= 1.01 && self.makeup_db.abs() < 0.01
    }
}

fn default_ratio() -> f32 {
    1.0
}

fn default_attack_ms() -> f32 {
    DEFAULT_ATTACK_MS
}

fn default_release_ms() -> f32 {
    DEFAULT_RELEASE_MS
}

fn is_bypass_ratio(value: &f32) -> bool {
    *value <= 1.01
}

fn is_default_threshold(value: &f32) -> bool {
    value.abs() < 0.01
}

fn is_default_attack(value: &f32) -> bool {
    (*value - DEFAULT_ATTACK_MS).abs() < 0.05
}

fn is_default_release(value: &f32) -> bool {
    (*value - DEFAULT_RELEASE_MS).abs() < 0.5
}

fn is_zero_db(value: &f32) -> bool {
    value.abs() < 0.01
}

pub fn clamp_threshold_db(db: f32) -> f32 {
    if !db.is_finite() {
        return 0.0;
    }
    db.clamp(THRESHOLD_DB_MIN, THRESHOLD_DB_MAX)
}

pub fn clamp_ratio(ratio: f32) -> f32 {
    if !ratio.is_finite() {
        return 1.0;
    }
    ratio.clamp(RATIO_MIN, RATIO_MAX)
}

pub fn clamp_attack_ms(ms: f32) -> f32 {
    if !ms.is_finite() {
        return DEFAULT_ATTACK_MS;
    }
    ms.clamp(ATTACK_MS_MIN, ATTACK_MS_MAX)
}

pub fn clamp_release_ms(ms: f32) -> f32 {
    if !ms.is_finite() {
        return DEFAULT_RELEASE_MS;
    }
    ms.clamp(RELEASE_MS_MIN, RELEASE_MS_MAX)
}

pub fn clamp_makeup_db(db: f32) -> f32 {
    if !db.is_finite() {
        return 0.0;
    }
    db.clamp(MAKEUP_DB_MIN, MAKEUP_DB_MAX)
}

pub fn format_threshold_db(db: f32) -> String {
    let db = clamp_threshold_db(db);
    if db.abs() < 0.05 {
        "0".to_string()
    } else {
        format!("{db:+.0}")
    }
}

pub fn format_ratio(ratio: f32) -> String {
    let ratio = clamp_ratio(ratio);
    if ratio <= 1.01 {
        "1:1".to_string()
    } else if ratio >= 19.5 {
        "∞:1".to_string()
    } else {
        format!("{ratio:.0}:1")
    }
}

pub fn format_time_ms(ms: f32) -> String {
    let ms = ms.max(0.0);
    if ms < 10.0 {
        format!("{ms:.1}ms")
    } else {
        format!("{ms:.0}ms")
    }
}

pub fn format_makeup_db(db: f32) -> String {
    let db = clamp_makeup_db(db);
    if db.abs() < 0.05 {
        "0".to_string()
    } else {
        format!("{db:+.0}")
    }
}

fn amp_to_db(amp: f32) -> f32 {
    if !amp.is_finite() || amp <= 1.0e-8 {
        return THRESHOLD_DB_MIN;
    }
    20.0 * amp.log10()
}

fn envelope_coeff(time_ms: f32, sample_rate: f32) -> f32 {
    let time_ms = time_ms.max(0.01);
    let tau = time_ms * 0.001 * sample_rate;
    (-1.0 / tau).exp()
}

fn static_gain_reduction_db(level_db: f32, threshold_db: f32, ratio: f32) -> f32 {
    if ratio <= 1.01 || level_db <= threshold_db {
        return 0.0;
    }
    let excess = level_db - threshold_db;
    excess / ratio - excess
}

/// ffmpeg `acompressor` filter for one stereo branch. Empty when bypassed.
pub fn ffmpeg_compressor_filter(comp: &TrackCompressor) -> Option<String> {
    if comp.is_bypass() {
        return None;
    }
    let threshold = db_to_linear(clamp_threshold_db(comp.threshold_db)).clamp(0.000975, 1.0);
    let ratio = clamp_ratio(comp.ratio);
    let attack = clamp_attack_ms(comp.attack_ms);
    let release = clamp_release_ms(comp.release_ms);
    let makeup = db_to_linear(clamp_makeup_db(comp.makeup_db)).clamp(1.0, 64.0);
    Some(format!(
        "acompressor=threshold={threshold:.6}:ratio={ratio:.2}:attack={attack:.2}:release={release:.2}:makeup={makeup:.4}"
    ))
}

/// Stateful stereo-linked compressor for real-time playback.
#[derive(Clone, Debug)]
pub struct CompressorProcessor {
    params: TrackCompressor,
    gain_db: f32,
    attack_coeff: f32,
    release_coeff: f32,
    makeup_linear: f32,
}

impl Default for CompressorProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl CompressorProcessor {
    pub fn new() -> Self {
        let mut processor = Self {
            params: TrackCompressor::default(),
            gain_db: 0.0,
            attack_coeff: 0.0,
            release_coeff: 0.0,
            makeup_linear: 1.0,
        };
        processor.apply_params(TrackCompressor::default());
        processor
    }

    pub fn params(&self) -> TrackCompressor {
        self.params
    }

    pub fn set_params(&mut self, params: TrackCompressor) {
        let next = TrackCompressor {
            threshold_db: clamp_threshold_db(params.threshold_db),
            ratio: clamp_ratio(params.ratio),
            attack_ms: clamp_attack_ms(params.attack_ms),
            release_ms: clamp_release_ms(params.release_ms),
            makeup_db: clamp_makeup_db(params.makeup_db),
        };
        if next == self.params {
            return;
        }
        self.apply_params(next);
    }

    fn apply_params(&mut self, params: TrackCompressor) {
        self.params = params;
        self.attack_coeff = envelope_coeff(params.attack_ms, COMP_SAMPLE_RATE);
        self.release_coeff = envelope_coeff(params.release_ms, COMP_SAMPLE_RATE);
        self.makeup_linear = db_to_linear(params.makeup_db);
    }

    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        if self.params.is_bypass() {
            return (left, right);
        }
        let l = if left.is_finite() { left } else { 0.0 };
        let r = if right.is_finite() { right } else { 0.0 };
        let level_db = amp_to_db(l.abs().max(r.abs()));
        let target = static_gain_reduction_db(
            level_db,
            self.params.threshold_db,
            self.params.ratio,
        );
        let coeff = if target < self.gain_db {
            self.attack_coeff
        } else {
            self.release_coeff
        };
        self.gain_db = target + coeff * (self.gain_db - target);
        let gain = db_to_linear(self.gain_db) * self.makeup_linear;
        (l * gain, r * gain)
    }

    pub fn reset(&mut self) {
        self.gain_db = 0.0;
    }
}

/// Process a block of interleaved stereo samples in place.
pub fn process_interleaved(samples: &mut [f32], processor: &mut CompressorProcessor) {
    for frame in samples.chunks_exact_mut(2) {
        let (l, r) = processor.process(frame[0], frame[1]);
        frame[0] = l;
        frame[1] = r;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle_peak(params: TrackCompressor, level: f32, frames: usize) -> f32 {
        let mut processor = CompressorProcessor::new();
        processor.set_params(params);
        let mut peak: f32 = 0.0;
        for index in 0..frames {
            let sample = if index < frames / 4 {
                0.0
            } else {
                level
            };
            let (left, _) = processor.process(sample, sample);
            if index >= frames * 3 / 4 {
                peak = peak.max(left.abs());
            }
        }
        peak
    }

    #[test]
    fn defaults_are_bypass_and_flat_json() {
        let comp = TrackCompressor::default();
        assert!(comp.is_bypass());
        let json = serde_json::to_string(&comp).unwrap();
        assert_eq!(json, "{}");
        assert!(ffmpeg_compressor_filter(&comp).is_none());
    }

    #[test]
    fn compression_reduces_hot_signal() {
        let input = 0.9;
        let flat = settle_peak(TrackCompressor::default(), input, 24_576);
        let compressed = settle_peak(
            TrackCompressor {
                threshold_db: -12.0,
                ratio: 4.0,
                attack_ms: 1.0,
                release_ms: 200.0,
                makeup_db: 0.0,
            },
            input,
            24_576,
        );
        assert!(
            compressed < flat * 0.75,
            "compressed {compressed} vs flat {flat}"
        );
    }

    #[test]
    fn makeup_restores_level() {
        let input = 0.8;
        let with_makeup = settle_peak(
            TrackCompressor {
                threshold_db: -18.0,
                ratio: 4.0,
                attack_ms: 1.0,
                release_ms: 200.0,
                makeup_db: 6.0,
            },
            input,
            24_576,
        );
        let without_makeup = settle_peak(
            TrackCompressor {
                threshold_db: -18.0,
                ratio: 4.0,
                attack_ms: 1.0,
                release_ms: 200.0,
                makeup_db: 0.0,
            },
            input,
            24_576,
        );
        assert!(
            with_makeup > without_makeup * 1.5,
            "makeup {with_makeup} vs dry {without_makeup}"
        );
    }

    #[test]
    fn interleaved_block_matches_sample_by_sample() {
        let mut block = vec![0.0; 512];
        for index in 0..256 {
            let sample = ((index as f32 / 40.0).sin() * 0.7).clamp(-0.95, 0.95);
            block[index * 2] = sample;
            block[index * 2 + 1] = sample * 0.8;
        }
        let reference = block.clone();
        let params = TrackCompressor {
            threshold_db: -20.0,
            ratio: 3.0,
            attack_ms: 5.0,
            release_ms: 80.0,
            makeup_db: 2.0,
        };
        let mut a = CompressorProcessor::new();
        let mut b = CompressorProcessor::new();
        a.set_params(params);
        b.set_params(params);
        process_interleaved(&mut block, &mut a);
        for frame in 0..256 {
            let (l, r) = b.process(reference[frame * 2], reference[frame * 2 + 1]);
            assert!((block[frame * 2] - l).abs() < 1.0e-5);
            assert!((block[frame * 2 + 1] - r).abs() < 1.0e-5);
        }
    }

    #[test]
    fn ffmpeg_filter_includes_parameters() {
        let comp = TrackCompressor {
            threshold_db: -18.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 120.0,
            makeup_db: 3.0,
        };
        let filter = ffmpeg_compressor_filter(&comp).unwrap();
        assert!(filter.contains("acompressor="));
        assert!(filter.contains("ratio=4.00"));
        assert!(filter.contains("attack=10.00"));
        assert!(filter.contains("release=120.00"));
    }

    #[test]
    fn track_compressor_round_trips_in_json() {
        let comp = TrackCompressor {
            threshold_db: -24.0,
            ratio: 6.0,
            attack_ms: 5.0,
            release_ms: 250.0,
            makeup_db: 4.0,
        };
        let json = serde_json::to_string(&comp).unwrap();
        let loaded: TrackCompressor = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, comp);
    }

    #[test]
    fn clamp_and_format_helpers() {
        assert_eq!(clamp_threshold_db(99.0), THRESHOLD_DB_MAX);
        assert_eq!(clamp_ratio(0.5), RATIO_MIN);
        assert_eq!(format_ratio(1.0), "1:1");
        assert_eq!(format_ratio(4.2), "4:1");
        assert_eq!(format_threshold_db(-12.0), "-12");
        assert_eq!(format_makeup_db(0.0), "0");
    }
}
