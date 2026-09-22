//! Per-track 3-band EQ shared by playback and export.
//!
//! Low and high are shelves; mid is a peaking band. An optional low-cut is a
//! second-order high-pass. Coefficients follow the RBJ cookbook at 48 kHz.

use serde::{Deserialize, Serialize};

pub const EQ_DB_MIN: f32 = -12.0;
pub const EQ_DB_MAX: f32 = 12.0;
pub const EQ_SAMPLE_RATE: f32 = 48_000.0;

pub const EQ_LOW_HZ: f32 = 200.0;
pub const EQ_MID_HZ: f32 = 1_000.0;
pub const EQ_MID_Q: f32 = 1.0;
pub const EQ_HIGH_HZ: f32 = 8_000.0;
pub const EQ_LOW_CUT_HZ: f32 = 80.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackEq3 {
    /// Low shelf gain in dB.
    #[serde(default, skip_serializing_if = "is_zero_db")]
    pub low: f32,
    /// Mid peaking gain in dB.
    #[serde(default, skip_serializing_if = "is_zero_db")]
    pub mid: f32,
    /// High shelf gain in dB.
    #[serde(default, skip_serializing_if = "is_zero_db")]
    pub high: f32,
    /// Second-order high-pass at [`EQ_LOW_CUT_HZ`].
    #[serde(default, skip_serializing_if = "is_false")]
    pub low_cut: bool,
}

impl Default for TrackEq3 {
    fn default() -> Self {
        Self {
            low: 0.0,
            mid: 0.0,
            high: 0.0,
            low_cut: false,
        }
    }
}

impl TrackEq3 {
    pub fn is_bypass(&self) -> bool {
        self.low.abs() < 0.01 && self.mid.abs() < 0.01 && self.high.abs() < 0.01 && !self.low_cut
    }
}

fn is_zero_db(value: &f32) -> bool {
    value.abs() < 0.01
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub fn clamp_eq_db(db: f32) -> f32 {
    if !db.is_finite() {
        return 0.0;
    }
    db.clamp(EQ_DB_MIN, EQ_DB_MAX)
}

pub fn format_eq_db(db: f32) -> String {
    let db = clamp_eq_db(db);
    if db.abs() < 0.05 {
        "0".to_string()
    } else {
        format!("{db:+.0}")
    }
}

/// ffmpeg filter fragments for one stereo branch. Empty when bypassed.
pub fn ffmpeg_eq_filters(eq: &TrackEq3) -> Vec<String> {
    if eq.is_bypass() {
        return Vec::new();
    }
    let mut filters = Vec::new();
    if eq.low_cut {
        filters.push(format!("highpass=f={EQ_LOW_CUT_HZ:.0}"));
    }
    if eq.low.abs() >= 0.01 {
        filters.push(format!(
            "equalizer=f={EQ_LOW_HZ:.0}:width_type=h:width=2:gain={:.1}",
            eq.low
        ));
    }
    if eq.mid.abs() >= 0.01 {
        filters.push(format!(
            "equalizer=f={EQ_MID_HZ:.0}:width_type=q:width={EQ_MID_Q:.1}:gain={:.1}",
            eq.mid
        ));
    }
    if eq.high.abs() >= 0.01 {
        filters.push(format!(
            "equalizer=f={EQ_HIGH_HZ:.0}:width_type=h:width=2:gain={:.1}",
            eq.high
        ));
    }
    filters
}

#[derive(Clone, Copy, Debug, Default)]
struct BiquadCoeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

#[derive(Clone, Debug, Default)]
struct Biquad {
    coeffs: BiquadCoeffs,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn set_coeffs(&mut self, coeffs: BiquadCoeffs) {
        self.coeffs = coeffs;
    }

    fn process(&mut self, input: f32) -> f32 {
        let x = if input.is_finite() { input } else { 0.0 };
        let c = self.coeffs;
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y
    }

    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

fn normalize(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> BiquadCoeffs {
    BiquadCoeffs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

fn low_shelf_coeffs(freq: f32, gain_db: f32, sample_rate: f32) -> BiquadCoeffs {
    let a = 10f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
    let cos = w0.cos();
    let sin = w0.sin();
    let alpha = sin / 2.0 * ((a + 1.0 / a) * (1.0 / 0.707 - 1.0) + 2.0).sqrt();
    let ap1 = a + 1.0;
    let am1 = a - 1.0;
    let b0 = a * (ap1 - am1 * cos + 2.0 * a.sqrt() * alpha);
    let b1 = 2.0 * a * (am1 - ap1 * cos);
    let b2 = a * (ap1 - am1 * cos - 2.0 * a.sqrt() * alpha);
    let a0 = ap1 + am1 * cos + 2.0 * a.sqrt() * alpha;
    let a1 = -2.0 * (am1 + ap1 * cos);
    let a2 = ap1 + am1 * cos - 2.0 * a.sqrt() * alpha;
    normalize(b0, b1, b2, a0, a1, a2)
}

fn high_shelf_coeffs(freq: f32, gain_db: f32, sample_rate: f32) -> BiquadCoeffs {
    let a = 10f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
    let cos = w0.cos();
    let sin = w0.sin();
    let alpha = sin / 2.0 * ((a + 1.0 / a) * (1.0 / 0.707 - 1.0) + 2.0).sqrt();
    let ap1 = a + 1.0;
    let am1 = a - 1.0;
    let b0 = a * (ap1 + am1 * cos + 2.0 * a.sqrt() * alpha);
    let b1 = -2.0 * a * (am1 + ap1 * cos);
    let b2 = a * (ap1 + am1 * cos - 2.0 * a.sqrt() * alpha);
    let a0 = ap1 - am1 * cos + 2.0 * a.sqrt() * alpha;
    let a1 = 2.0 * (am1 - ap1 * cos);
    let a2 = ap1 - am1 * cos - 2.0 * a.sqrt() * alpha;
    normalize(b0, b1, b2, a0, a1, a2)
}

fn peaking_coeffs(freq: f32, q: f32, gain_db: f32, sample_rate: f32) -> BiquadCoeffs {
    let a = 10f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
    let cos = w0.cos();
    let sin = w0.sin();
    let alpha = sin / (2.0 * q.max(0.1));
    let b0 = 1.0 + alpha * a;
    let b1 = -2.0 * cos;
    let b2 = 1.0 - alpha * a;
    let a0 = 1.0 + alpha / a;
    let a1 = -2.0 * cos;
    let a2 = 1.0 - alpha / a;
    normalize(b0, b1, b2, a0, a1, a2)
}

fn highpass_coeffs(freq: f32, sample_rate: f32) -> BiquadCoeffs {
    let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
    let cos = w0.cos();
    let sin = w0.sin();
    let alpha = sin / (2.0 * 0.707);
    let b0 = (1.0 + cos) / 2.0;
    let b1 = -(1.0 + cos);
    let b2 = (1.0 + cos) / 2.0;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cos;
    let a2 = 1.0 - alpha;
    normalize(b0, b1, b2, a0, a1, a2)
}

fn bypass_coeffs() -> BiquadCoeffs {
    BiquadCoeffs {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    }
}

/// Stateful stereo EQ for real-time playback.
#[derive(Clone, Debug)]
pub struct EqProcessor {
    params: TrackEq3,
    low_cut_l: Biquad,
    low_cut_r: Biquad,
    low_l: Biquad,
    low_r: Biquad,
    mid_l: Biquad,
    mid_r: Biquad,
    high_l: Biquad,
    high_r: Biquad,
}

impl Default for EqProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl EqProcessor {
    pub fn new() -> Self {
        let mut processor = Self {
            params: TrackEq3::default(),
            low_cut_l: Biquad::default(),
            low_cut_r: Biquad::default(),
            low_l: Biquad::default(),
            low_r: Biquad::default(),
            mid_l: Biquad::default(),
            mid_r: Biquad::default(),
            high_l: Biquad::default(),
            high_r: Biquad::default(),
        };
        processor.apply_params(TrackEq3::default());
        processor
    }

    pub fn params(&self) -> TrackEq3 {
        self.params
    }

    pub fn set_params(&mut self, params: TrackEq3) {
        let next = TrackEq3 {
            low: clamp_eq_db(params.low),
            mid: clamp_eq_db(params.mid),
            high: clamp_eq_db(params.high),
            low_cut: params.low_cut,
        };
        if next == self.params {
            return;
        }
        self.apply_params(next);
    }

    fn apply_params(&mut self, params: TrackEq3) {
        self.params = params;
        if params.low_cut {
            let coeffs = highpass_coeffs(EQ_LOW_CUT_HZ, EQ_SAMPLE_RATE);
            self.low_cut_l.set_coeffs(coeffs);
            self.low_cut_r.set_coeffs(coeffs);
        } else {
            let bypass = bypass_coeffs();
            self.low_cut_l.set_coeffs(bypass);
            self.low_cut_r.set_coeffs(bypass);
        }
        let low = if params.low.abs() >= 0.01 {
            low_shelf_coeffs(EQ_LOW_HZ, params.low, EQ_SAMPLE_RATE)
        } else {
            bypass_coeffs()
        };
        self.low_l.set_coeffs(low);
        self.low_r.set_coeffs(low);
        let mid = if params.mid.abs() >= 0.01 {
            peaking_coeffs(EQ_MID_HZ, EQ_MID_Q, params.mid, EQ_SAMPLE_RATE)
        } else {
            bypass_coeffs()
        };
        self.mid_l.set_coeffs(mid);
        self.mid_r.set_coeffs(mid);
        let high = if params.high.abs() >= 0.01 {
            high_shelf_coeffs(EQ_HIGH_HZ, params.high, EQ_SAMPLE_RATE)
        } else {
            bypass_coeffs()
        };
        self.high_l.set_coeffs(high);
        self.high_r.set_coeffs(high);
    }

    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        if self.params.is_bypass() {
            return (left, right);
        }
        let mut l = left;
        let mut r = right;
        if self.params.low_cut {
            l = self.low_cut_l.process(l);
            r = self.low_cut_r.process(r);
        }
        if self.params.low.abs() >= 0.01 {
            l = self.low_l.process(l);
            r = self.low_r.process(r);
        }
        if self.params.mid.abs() >= 0.01 {
            l = self.mid_l.process(l);
            r = self.mid_r.process(r);
        }
        if self.params.high.abs() >= 0.01 {
            l = self.high_l.process(l);
            r = self.high_r.process(r);
        }
        (l, r)
    }

    pub fn reset(&mut self) {
        self.low_cut_l.reset();
        self.low_cut_r.reset();
        self.low_l.reset();
        self.low_r.reset();
        self.mid_l.reset();
        self.mid_r.reset();
        self.high_l.reset();
        self.high_r.reset();
    }
}

/// Process a block of interleaved stereo samples in place.
pub fn process_interleaved(samples: &mut [f32], processor: &mut EqProcessor) {
    for frame in samples.chunks_exact_mut(2) {
        let (l, r) = processor.process(frame[0], frame[1]);
        frame[0] = l;
        frame[1] = r;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, sample_rate: f32, frames: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * 2);
        for index in 0..frames {
            let t = index as f32 / sample_rate;
            let value = (2.0 * std::f32::consts::PI * freq * t).sin() * 0.5;
            out.push(value);
            out.push(value);
        }
        out
    }

    #[test]
    fn defaults_are_bypass_and_flat_json() {
        let eq = TrackEq3::default();
        assert!(eq.is_bypass());
        let json = serde_json::to_string(&eq).unwrap();
        assert_eq!(json, "{}");
        assert!(ffmpeg_eq_filters(&eq).is_empty());
    }

    fn settle(params: TrackEq3, freq: f32) -> f32 {
        let mut processor = EqProcessor::new();
        processor.set_params(params);
        let mut peak: f32 = 0.0;
        for index in 0..16_384 {
            let t = index as f32 / EQ_SAMPLE_RATE;
            let sample = (2.0 * std::f32::consts::PI * freq * t).sin();
            let (left, _) = processor.process(sample, sample);
            if index >= 12_288 {
                peak = peak.max(left.abs());
            }
        }
        peak
    }

    #[test]
    fn low_shelf_boosts_bass_and_cut_attenuates_sub() {
        let bass = settle(TrackEq3::default(), 60.0);
        let boosted = settle(
            TrackEq3 {
                low: 6.0,
                ..Default::default()
            },
            60.0,
        );
        assert!(boosted > bass * 1.3, "bass {boosted} vs flat {bass}");

        let sub = settle(
            TrackEq3 {
                low_cut: true,
                ..Default::default()
            },
            35.0,
        );
        let mid = settle(
            TrackEq3 {
                low_cut: true,
                ..Default::default()
            },
            1_000.0,
        );
        assert!(sub < mid * 0.35, "sub {sub} vs mid {mid}");
    }

    #[test]
    fn mid_and_high_bands_move_their_regions() {
        let flat_mid = settle(TrackEq3::default(), 1_000.0);
        let boosted_mid = settle(
            TrackEq3 {
                mid: 6.0,
                ..Default::default()
            },
            1_000.0,
        );
        assert!(boosted_mid > flat_mid * 1.3, "{boosted_mid} vs {flat_mid}");

        let flat_high = settle(TrackEq3::default(), 12_000.0);
        let boosted_high = settle(
            TrackEq3 {
                high: 6.0,
                ..Default::default()
            },
            12_000.0,
        );
        assert!(boosted_high > flat_high * 1.3, "{boosted_high} vs {flat_high}");
    }

    #[test]
    fn interleaved_block_matches_sample_by_sample() {
        let mut block = sine(440.0, EQ_SAMPLE_RATE, 256);
        let reference = block.clone();
        let mut a = EqProcessor::new();
        let mut b = EqProcessor::new();
        a.set_params(TrackEq3 {
            low: 3.0,
            mid: -2.0,
            high: 4.0,
            low_cut: true,
        });
        b.set_params(a.params());
        process_interleaved(&mut block, &mut a);
        for frame in 0..256 {
            let (l, r) = b.process(reference[frame * 2], reference[frame * 2 + 1]);
            assert!((block[frame * 2] - l).abs() < 1.0e-5);
            assert!((block[frame * 2 + 1] - r).abs() < 1.0e-5);
        }
    }

    #[test]
    fn ffmpeg_filters_include_bands_and_low_cut() {
        let eq = TrackEq3 {
            low: 3.0,
            mid: -2.0,
            high: 4.5,
            low_cut: true,
        };
        let filters = ffmpeg_eq_filters(&eq);
        assert_eq!(filters.len(), 4);
        assert!(filters[0].contains("highpass"));
        assert!(filters[1].contains("f=200"));
        assert!(filters[2].contains("f=1000"));
        assert!(filters[3].contains("f=8000"));
    }

    #[test]
    fn track_eq_round_trips_in_json() {
        let eq = TrackEq3 {
            low: 2.5,
            mid: -1.0,
            high: 0.0,
            low_cut: true,
        };
        let json = serde_json::to_string(&eq).unwrap();
        let loaded: TrackEq3 = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, eq);
    }

    #[test]
    fn clamp_and_format_helpers() {
        assert_eq!(clamp_eq_db(99.0), EQ_DB_MAX);
        assert_eq!(clamp_eq_db(-99.0), EQ_DB_MIN);
        assert_eq!(format_eq_db(0.0), "0");
        assert_eq!(format_eq_db(3.2), "+3");
    }
}
