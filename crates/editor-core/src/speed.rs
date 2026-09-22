//! Source-time mapping for constant speed and a linear ramp.
//!
//! Speed is source-seconds per timeline-second. `source_frame_at` uses this
//! module whenever a clip is not 100% forward. The integral of the rate from
//! the clip in-point is the source offset. Reverse walks that same distance
//! back from the source out-point.

use crate::effects::Interpolation;
use crate::model::{clamp_speed, Clip, ClipSpeed, SPEED_MAX, SPEED_MIN};
use crate::time::{Frame, Timebase};

impl ClipSpeed {
    /// Speed at the first frame of the clip.
    pub fn start_rate(&self) -> f32 {
        if let Some(key) = self.rate.keys.iter().find(|key| key.frame == 0) {
            return clamp_speed(key.value);
        }
        if self.rate.keys.is_empty() {
            return clamp_speed(self.rate.base);
        }
        let mut earliest: Option<&crate::effects::KeyframeF32> = None;
        for key in &self.rate.keys {
            if key.frame < 0 {
                continue;
            }
            earliest = Some(match earliest {
                Some(current) if current.frame <= key.frame => current,
                _ => key,
            });
        }
        earliest
            .map(|key| clamp_speed(key.value))
            .unwrap_or_else(|| clamp_speed(self.rate.base))
    }

    /// End speed when the curve has a key at frame `-1` (the clip out point).
    pub fn ramp_end(&self) -> Option<f32> {
        self.rate
            .keys
            .iter()
            .find(|key| key.frame < 0)
            .map(|key| clamp_speed(key.value))
    }

    /// Mean rate across `duration` sequence frames. A 0→−1 ramp is `(start+end)/2`.
    pub fn average_rate(&self, duration: i64) -> f64 {
        let duration = duration.max(1);
        if self.rate.keys.is_empty() {
            return f64::from(clamp_speed(self.rate.base));
        }
        let integral = integrate_curve(&self.rate, duration, duration);
        let average = integral / duration as f64;
        if !average.is_finite() {
            return 1.0;
        }
        average.clamp(f64::from(SPEED_MIN), f64::from(SPEED_MAX))
    }

    /// Retimed audio is muted. A constant 100% forward clip keeps its sound.
    /// A flat 100–100 ramp is still realtime, so it stays audible.
    pub fn mutes_audio(&self) -> bool {
        if self.reverse {
            return true;
        }
        if self.rate.keys.is_empty() {
            return (clamp_speed(self.rate.base) - 1.0).abs() > 1.0e-3;
        }
        self.rate
            .keys
            .iter()
            .any(|key| (clamp_speed(key.value) - 1.0).abs() > 1.0e-3)
    }

    /// Short label for the timeline and the program monitor. `None` at 100%.
    pub fn badge(&self) -> Option<String> {
        if self.is_identity() {
            return None;
        }
        let start = (self.start_rate() * 100.0).round() as i32;
        let mut text = match self.ramp_end() {
            Some(end) => {
                let end = (end * 100.0).round() as i32;
                if end == start {
                    format!("{start}%")
                } else {
                    format!("{start}–{end}%")
                }
            }
            None => format!("{start}%"),
        };
        if self.reverse {
            text.push_str(" Rev");
        }
        Some(text)
    }
}

/// Timeline frames that consume `source_span` media frames at `rate`.
pub fn timeline_span_for_speed(
    source_span: i64,
    rate: f64,
    media: Timebase,
    sequence: Timebase,
) -> i64 {
    let source_span = source_span.max(1);
    let rate = if rate.is_finite() {
        rate.clamp(f64::from(SPEED_MIN), f64::from(SPEED_MAX))
    } else {
        1.0
    };
    if !media.is_valid() || !sequence.is_valid() {
        return source_span;
    }
    let raw = source_span as f64 * sequence.fps_f64() / (rate * media.fps_f64());
    quantize_span(raw).max(1)
}

/// Source frames corresponding to `timeline_delta` sequence frames at a constant `rate`.
pub fn source_frames_for_rate(
    timeline_delta: i64,
    rate: f64,
    sequence: Timebase,
    media: Timebase,
) -> i64 {
    if timeline_delta == 0 || !sequence.is_valid() || !media.is_valid() {
        return 0;
    }
    let rate = if rate.is_finite() {
        rate.clamp(f64::from(SPEED_MIN), f64::from(SPEED_MAX))
    } else {
        1.0
    };
    let magnitude =
        (timeline_delta.unsigned_abs() as f64) * rate * media.fps_f64() / sequence.fps_f64();
    let frames = quantize_floor(magnitude);
    if timeline_delta < 0 {
        -frames
    } else {
        frames
    }
}

/// Media frame of `clip` at `timeline_frame`. Holds the first or last included
/// source frame once the integral leaves the source range.
pub fn mapped_source_frame(clip: &Clip, timeline_frame: Frame, sequence: Timebase) -> Frame {
    let span = clip.source_duration();
    if span <= 0 {
        return clip.source_in;
    }
    let delta = timeline_frame.0 - clip.timeline_in.0;
    if delta <= 0 {
        return if clip.speed.reverse {
            Frame(clip.source_out.0.saturating_sub(1))
        } else {
            clip.source_in
        };
    }
    let duration = clip.duration().max(1);
    let mut consumed = consumed_source_frames(clip, delta, duration, sequence);
    if consumed >= span {
        consumed = span - 1;
    }
    if clip.speed.reverse {
        Frame(clip.source_out.0 - 1 - consumed)
    } else {
        Frame(clip.source_in.0 + consumed)
    }
}

fn consumed_source_frames(clip: &Clip, delta: i64, duration: i64, sequence: Timebase) -> i64 {
    if delta <= 0 || !sequence.is_valid() || !clip.media_timebase.is_valid() {
        return 0;
    }
    let integral = integrate_curve(&clip.speed.rate, delta, duration);
    let source_seconds = integral * sequence.frame_duration_secs();
    quantize_floor(source_seconds * clip.media_timebase.fps_f64()).max(0)
}

struct ResolvedKey {
    frame: i64,
    value: f64,
    linear: bool,
}

fn resolved_keys(rate: &crate::effects::AnimatedF32, duration: i64) -> Vec<ResolvedKey> {
    let end = duration.max(1);
    let mut indexed: Vec<(usize, ResolvedKey)> = rate
        .keys
        .iter()
        .enumerate()
        .map(|(index, key)| {
            let frame = if key.frame < 0 { end } else { key.frame };
            (
                index,
                ResolvedKey {
                    frame,
                    value: f64::from(clamp_speed(key.value)),
                    linear: key.interpolation == Interpolation::Linear,
                },
            )
        })
        .collect();
    indexed.sort_by_key(|(index, key)| (key.frame, *index));
    let mut out = Vec::new();
    for (_, key) in indexed {
        if out
            .last()
            .is_some_and(|prev: &ResolvedKey| prev.frame == key.frame)
        {
            out.pop();
        }
        out.push(key);
    }
    out
}

/// ∫ rate dt from 0 to `delta`, with `t` in sequence frames.
fn integrate_curve(rate: &crate::effects::AnimatedF32, delta: i64, duration: i64) -> f64 {
    if delta <= 0 {
        return 0.0;
    }
    let keys = resolved_keys(rate, duration);
    if keys.is_empty() {
        return f64::from(clamp_speed(rate.base)) * delta as f64;
    }
    let mut acc = 0.0;
    let mut cursor = 0i64;
    let first = keys[0].frame;
    if cursor < first.min(delta) {
        let end = first.min(delta);
        acc += (end - cursor) as f64 * keys[0].value;
        cursor = end;
    }
    for pair in keys.windows(2) {
        if cursor >= delta {
            break;
        }
        let left = &pair[0];
        let right = &pair[1];
        if right.frame <= cursor {
            continue;
        }
        let seg_start = cursor.max(left.frame);
        let seg_end = delta.min(right.frame);
        if seg_end <= seg_start {
            continue;
        }
        let span = (seg_end - seg_start) as f64;
        if left.linear && right.frame > left.frame {
            let denom = (right.frame - left.frame) as f64;
            let start_t = (seg_start - left.frame) as f64 / denom;
            let end_t = (seg_end - left.frame) as f64 / denom;
            let va = left.value + (right.value - left.value) * start_t;
            let vb = left.value + (right.value - left.value) * end_t;
            acc += span * (va + vb) / 2.0;
        } else {
            acc += span * left.value;
        }
        cursor = seg_end;
    }
    if cursor < delta {
        acc += (delta - cursor) as f64 * keys.last().map(|key| key.value).unwrap_or(1.0);
    }
    acc
}

fn quantize_floor(value: f64) -> i64 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    (value + 1.0e-9).floor() as i64
}

fn quantize_span(raw: f64) -> i64 {
    if !raw.is_finite() || raw <= 0.0 {
        return 1;
    }
    let nearest = raw.round();
    let span = if (raw - nearest).abs() < 1.0e-6 {
        nearest
    } else {
        raw.ceil()
    };
    span.max(1.0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Clip;
    use crate::time::Timebase;

    fn clip_span(timeline: i64, source: i64) -> Clip {
        let mut clip = Clip::basic(1, 0, timeline);
        clip.source_out = Frame(source);
        clip.source_max = Frame(source.max(timeline));
        clip
    }

    #[test]
    fn constant_speed_steps_and_holds_source_frames() {
        let tb = Timebase::fps_24();
        let mut fast = clip_span(24, 48);
        fast.speed = ClipSpeed::constant(2.0, false);
        assert_eq!(mapped_source_frame(&fast, Frame(0), tb).0, 0);
        assert_eq!(mapped_source_frame(&fast, Frame(1), tb).0, 2);
        assert_eq!(mapped_source_frame(&fast, Frame(12), tb).0, 24);
        assert_eq!(mapped_source_frame(&fast, Frame(23), tb).0, 46);

        let mut slow = clip_span(48, 24);
        slow.speed = ClipSpeed::constant(0.5, false);
        assert_eq!(mapped_source_frame(&slow, Frame(0), tb).0, 0);
        assert_eq!(mapped_source_frame(&slow, Frame(1), tb).0, 0);
        assert_eq!(mapped_source_frame(&slow, Frame(2), tb).0, 1);
        assert_eq!(mapped_source_frame(&slow, Frame(3), tb).0, 1);

        let mut quarter = clip_span(48, 12);
        quarter.speed = ClipSpeed::constant(0.25, false);
        assert_eq!(mapped_source_frame(&quarter, Frame(3), tb).0, 0);
        assert_eq!(mapped_source_frame(&quarter, Frame(4), tb).0, 1);

        let mut quad = clip_span(12, 48);
        quad.speed = ClipSpeed::constant(4.0, false);
        assert_eq!(mapped_source_frame(&quad, Frame(1), tb).0, 4);
        assert_eq!(mapped_source_frame(&quad, Frame(3), tb).0, 12);
    }

    #[test]
    fn reverse_walks_back_from_the_source_out_point() {
        let tb = Timebase::fps_24();
        let mut clip = clip_span(24, 24);
        clip.speed = ClipSpeed::constant(1.0, true);
        assert_eq!(mapped_source_frame(&clip, Frame(0), tb).0, 23);
        assert_eq!(mapped_source_frame(&clip, Frame(1), tb).0, 22);
        assert_eq!(mapped_source_frame(&clip, Frame(23), tb).0, 0);
        assert!(clip.speed.mutes_audio());
    }

    #[test]
    fn linear_ramp_integrates_between_the_two_speeds() {
        let tb = Timebase::fps_24();
        let mut clip = clip_span(24, 48);
        clip.speed = ClipSpeed::ramp(1.0, 3.0, false);
        assert!((clip.speed.average_rate(24) - 2.0).abs() < 1.0e-9);
        assert_eq!(mapped_source_frame(&clip, Frame(0), tb).0, 0);
        assert_eq!(mapped_source_frame(&clip, Frame(12), tb).0, 18);
        assert_eq!(timeline_span_for_speed(48, 2.0, tb, tb), 24);
        assert_eq!(clip.speed.badge().as_deref(), Some("100–300%"));
        assert!(clip.speed.mutes_audio());
    }

    #[test]
    fn speed_respects_a_different_media_timebase() {
        let sequence = Timebase::fps_24();
        let mut clip = Clip::basic(1, 0, 12);
        clip.media_timebase = Timebase::fps_30();
        clip.source_out = Frame(30);
        clip.source_max = Frame(90);
        clip.speed = ClipSpeed::constant(2.0, false);
        assert_eq!(
            timeline_span_for_speed(30, 2.0, clip.media_timebase, sequence),
            12
        );
        assert_eq!(mapped_source_frame(&clip, Frame(0), sequence).0, 0);
        assert_eq!(mapped_source_frame(&clip, Frame(6), sequence).0, 15);
    }

    #[test]
    fn rates_outside_the_range_clamp_and_unity_stays_audible() {
        let slow = ClipSpeed::constant(0.01, false);
        let fast = ClipSpeed::constant(12.0, false);
        assert!((slow.start_rate() - 0.25).abs() < 1.0e-6);
        assert!((fast.start_rate() - 4.0).abs() < 1.0e-6);
        assert!(!ClipSpeed::normal().mutes_audio());
        assert!(!ClipSpeed::ramp(1.0, 1.0, false).mutes_audio());
        assert!(ClipSpeed::constant(2.0, false).mutes_audio());
    }

    #[test]
    fn identity_speed_is_omitted_from_json_and_a_ramp_round_trips() {
        let clip = Clip::basic(1, 0, 24);
        let json = serde_json::to_string(&clip).unwrap();
        assert!(!json.contains("speed"));
        let loaded: Clip = serde_json::from_str(&json).unwrap();
        assert!(loaded.speed.is_identity());

        let legacy = r#"{"id":1,"name":"Clip 1","timeline_in":0,"timeline_out":24,"source_in":0,"source_out":24,"source_max":24}"#;
        let loaded: Clip = serde_json::from_str(legacy).unwrap();
        assert!(loaded.speed.is_identity());
        assert_eq!(
            crate::source_frame_at(&loaded, Frame(10), Timebase::fps_24()).0,
            10
        );

        let mut ramp = clip.clone();
        ramp.speed = ClipSpeed::ramp(0.5, 2.0, true);
        let json = serde_json::to_string(&ramp).unwrap();
        assert!(json.contains("\"frame\":-1"));
        let loaded: Clip = serde_json::from_str(&json).unwrap();
        assert!(loaded.speed.reverse);
        assert!((loaded.speed.start_rate() - 0.5).abs() < 1.0e-5);
        assert!((loaded.speed.ramp_end().unwrap() - 2.0).abs() < 1.0e-5);
        let again = serde_json::from_str::<Clip>(&serde_json::to_string(&loaded).unwrap()).unwrap();
        assert_eq!(again.speed, loaded.speed);
    }
}
