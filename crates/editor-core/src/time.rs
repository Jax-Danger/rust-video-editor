//! Rational timebase and frame-accurate positions.
//!
//! Frame indices on a [`Timebase`] are the source of truth. Floating-point
//! seconds exist only for display, playback pacing, and probe conversion.

use serde::{Deserialize, Serialize};

/// Frames per second as an exact ratio (`24000/1001`, `24/1`, `30000/1001`, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Timebase {
    pub numerator: u32,
    pub denominator: u32,
}

impl Timebase {
    pub const fn new(numerator: u32, denominator: u32) -> Self {
        Self {
            numerator,
            denominator,
        }
    }

    pub const fn fps_24() -> Self {
        Self::new(24, 1)
    }

    pub const fn fps_25() -> Self {
        Self::new(25, 1)
    }

    pub const fn fps_30() -> Self {
        Self::new(30, 1)
    }

    pub const fn fps_60() -> Self {
        Self::new(60, 1)
    }

    pub const fn fps_23976() -> Self {
        Self::new(24_000, 1001)
    }

    pub const fn fps_2997() -> Self {
        Self::new(30_000, 1001)
    }

    pub const fn fps_5994() -> Self {
        Self::new(60_000, 1001)
    }

    pub fn is_valid(self) -> bool {
        self.numerator > 0 && self.denominator > 0
    }

    /// Nominal frames per second. Display and pacing only.
    pub fn fps_f64(self) -> f64 {
        self.numerator as f64 / self.denominator as f64
    }

    /// Duration of one frame in seconds. Playback pacing only.
    pub fn frame_duration_secs(self) -> f64 {
        self.denominator as f64 / self.numerator as f64
    }

    /// Integer frame-number modulus used by timecode (24 for 23.976, 30 for 29.97).
    pub fn timecode_fps(self) -> i64 {
        let rounded = (i64::from(self.numerator) + i64::from(self.denominator) / 2)
            / i64::from(self.denominator);
        rounded.max(1)
    }

    /// SMPTE drop-frame nominal rate, if this timebase is 29.97 or 59.94.
    pub fn drop_frame_fps(self) -> Option<i64> {
        if self.denominator == 1001 && self.numerator == 30_000 {
            Some(30)
        } else if self.denominator == 1001 && self.numerator == 60_000 {
            Some(60)
        } else {
            None
        }
    }
}

impl Default for Timebase {
    fn default() -> Self {
        Self::fps_24()
    }
}

/// A frame index. Negative values are representable but edits reject them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Frame(pub i64);

impl Frame {
    pub const ZERO: Self = Self(0);

    pub fn saturating_add(self, delta: i64) -> Self {
        Self(self.0.saturating_add(delta))
    }

    pub fn to_seconds(self, timebase: Timebase) -> f64 {
        self.0 as f64 * timebase.frame_duration_secs()
    }

    /// Round a display duration to the nearest frame. Not a source of truth.
    pub fn from_seconds_round(seconds: f64, timebase: Timebase) -> Self {
        let frames = (seconds * timebase.fps_f64()).round();
        Self(frames as i64)
    }

    pub fn format_timecode(self, timebase: Timebase) -> String {
        format_timecode(self.0, timebase)
    }

    pub fn parse_timecode(text: &str, timebase: Timebase) -> Option<Self> {
        parse_timecode(text, timebase).map(Self)
    }
}

impl Default for Frame {
    fn default() -> Self {
        Self::ZERO
    }
}

/// A rational media timestamp: `ticks / timescale` seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MediaTime {
    pub ticks: i64,
    pub timescale: u32,
}

impl MediaTime {
    pub fn new(ticks: i64, timescale: u32) -> Self {
        Self { ticks, timescale }
    }

    pub fn from_frames(frames: i64, timebase: Timebase) -> Self {
        Self {
            ticks: frames.saturating_mul(i64::from(timebase.denominator)),
            timescale: timebase.numerator.max(1),
        }
    }

    pub fn to_frame_count(self, timebase: Timebase) -> i64 {
        if self.timescale == 0 || !timebase.is_valid() {
            return 0;
        }
        mul_div_round(
            self.ticks,
            i64::from(timebase.numerator),
            i64::from(self.timescale) * i64::from(timebase.denominator),
        )
    }

    pub fn to_seconds(self) -> f64 {
        if self.timescale == 0 {
            0.0
        } else {
            self.ticks as f64 / f64::from(self.timescale)
        }
    }
}

/// Convert `count` frames expressed in `from` into frames expressed in `to`.
pub fn convert_frames(count: i64, from: Timebase, to: Timebase) -> i64 {
    if count == 0 || !from.is_valid() || !to.is_valid() {
        return 0;
    }
    mul_div_round(
        count,
        i64::from(to.numerator) * i64::from(from.denominator),
        i64::from(to.denominator) * i64::from(from.numerator),
    )
}

/// `(a * b) / d` rounded to nearest, half away from zero via integer arithmetic.
pub fn mul_div_round(a: i64, b: i64, d: i64) -> i64 {
    if d == 0 {
        return 0;
    }
    let n = i128::from(a) * i128::from(b);
    let d = i128::from(d);
    let half = if n >= 0 { d.abs() / 2 } else { -(d.abs() / 2) };
    ((n + half) / d) as i64
}

fn format_timecode(frame: i64, timebase: Timebase) -> String {
    let sign = if frame < 0 { "-" } else { "" };
    let frame = frame.abs();
    if let Some(fps) = timebase.drop_frame_fps() {
        let (h, m, s, f) = frames_to_drop_components(frame, fps);
        format!("{sign}{h:02}:{m:02}:{s:02};{f:02}")
    } else {
        let fps = timebase.timecode_fps();
        let (h, m, s, f) = frames_to_nondrop_components(frame, fps);
        format!("{sign}{h:02}:{m:02}:{s:02}:{f:02}")
    }
}

fn parse_timecode(text: &str, timebase: Timebase) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let (sign, text) = if let Some(rest) = text.strip_prefix('-') {
        (-1, rest)
    } else {
        (1, text)
    };
    let drop_sep = text.contains(';');
    let parts: Vec<&str> = text.split(|c| c == ':' || c == ';').collect();
    if parts.len() != 4 {
        return None;
    }
    let h: i64 = parts[0].parse().ok()?;
    let m: i64 = parts[1].parse().ok()?;
    let s: i64 = parts[2].parse().ok()?;
    let f: i64 = parts[3].parse().ok()?;
    if m >= 60 || s >= 60 || h < 0 || m < 0 || s < 0 || f < 0 {
        return None;
    }
    let frames = if let Some(fps) = timebase.drop_frame_fps() {
        if !drop_sep && text.matches(':').count() == 3 {
            // Accept colon-separated input for drop-frame rates too.
        }
        drop_components_to_frames(h, m, s, f, fps)?
    } else {
        let fps = timebase.timecode_fps();
        if f >= fps {
            return None;
        }
        ((h * 60 + m) * 60 + s) * fps + f
    };
    Some(sign * frames)
}

fn frames_to_nondrop_components(frame: i64, fps: i64) -> (i64, i64, i64, i64) {
    let fps = fps.max(1);
    let ff = frame % fps;
    let secs = frame / fps;
    let ss = secs % 60;
    let mins = secs / 60;
    let mm = mins % 60;
    let hh = mins / 60;
    (hh, mm, ss, ff)
}

fn frames_to_drop_components(frame: i64, fps: i64) -> (i64, i64, i64, i64) {
    let drop = fps / 15;
    let frames_per_10 = fps * 60 * 10 - drop * 9;
    let frames_per_min = fps * 60 - drop;
    let d = frame.div_euclid(frames_per_10);
    let m = frame.rem_euclid(frames_per_10);
    let extra = if m < drop {
        0
    } else {
        drop * ((m - drop) / frames_per_min)
    };
    let adjusted = frame + drop * 9 * d + extra;
    frames_to_nondrop_components(adjusted, fps)
}

fn drop_components_to_frames(h: i64, m: i64, s: i64, f: i64, fps: i64) -> Option<i64> {
    let drop = fps / 15;
    if f >= fps {
        return None;
    }
    let total_minutes = h * 60 + m;
    if s == 0 && f < drop && total_minutes % 10 != 0 {
        return None;
    }
    let nominal = ((h * 3600 + m * 60 + s) * fps) + f;
    Some(nominal - drop * (total_minutes - total_minutes / 10))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nondrop_24_roundtrip() {
        let tb = Timebase::fps_24();
        for frame in [0, 1, 23, 24, 25, 24 * 60, 24 * 3600 + 24 * 61 + 3] {
            let text = Frame(frame).format_timecode(tb);
            assert_eq!(
                Frame::parse_timecode(&text, tb),
                Some(Frame(frame)),
                "{text}"
            );
        }
        assert_eq!(Frame(24).format_timecode(tb), "00:00:01:00");
        assert_eq!(Frame(90).format_timecode(Timebase::fps_30()), "00:00:03:00");
    }

    #[test]
    fn drop_frame_2997_vectors() {
        let tb = Timebase::fps_2997();
        assert_eq!(Frame(0).format_timecode(tb), "00:00:00;00");
        assert_eq!(Frame(1800).format_timecode(tb), "00:01:00;02");
        assert_eq!(Frame(17982).format_timecode(tb), "00:10:00;00");
        assert_eq!(Frame::parse_timecode("00:01:00;02", tb), Some(Frame(1800)));
        assert_eq!(Frame::parse_timecode("00:10:00;00", tb), Some(Frame(17982)));
        assert_eq!(Frame::parse_timecode("00:01:00;00", tb), None);
        assert_eq!(Frame::parse_timecode("00:01:00;01", tb), None);
        for frame in (0..20_000).step_by(17) {
            let text = Frame(frame).format_timecode(tb);
            assert_eq!(Frame::parse_timecode(&text, tb), Some(Frame(frame)));
        }
    }

    #[test]
    fn convert_frames_between_rates() {
        let from = Timebase::fps_30();
        let to = Timebase::fps_24();
        assert_eq!(convert_frames(30, from, to), 24);
        assert_eq!(convert_frames(24, to, from), 30);
        assert_eq!(
            convert_frames(100, Timebase::fps_24(), Timebase::fps_24()),
            100
        );
    }

    #[test]
    fn media_time_frame_roundtrip() {
        let tb = Timebase::fps_24();
        let mt = MediaTime::from_frames(144, tb);
        assert_eq!(mt.to_frame_count(tb), 144);
        let audio = Timebase::new(48_000, 1);
        let samples = MediaTime::new(240_000, 48_000);
        assert_eq!(samples.to_frame_count(tb), 120);
        assert_eq!(convert_frames(120, tb, audio), 240_000);
    }

    #[test]
    fn timecode_fps_rounds_ntsc() {
        assert_eq!(Timebase::fps_23976().timecode_fps(), 24);
        assert_eq!(Timebase::fps_2997().timecode_fps(), 30);
        assert_eq!(Timebase::fps_2997().drop_frame_fps(), Some(30));
        assert_eq!(Timebase::fps_24().drop_frame_fps(), None);
    }
}
