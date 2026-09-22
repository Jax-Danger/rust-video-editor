//! Timeline zoom and visibility for long sequences.
//!
//! Clips on a track are sorted by `timeline_in`. Drawing and hit-testing only
//! need the clips that intersect the viewport, which is a binary search plus
//! the handful of clips on screen — not a walk of every clip in an hour-long
//! cut.

use crate::model::Clip;
use crate::time::Frame;

/// Widest zoom: individual frames are easy to grab.
pub const MAX_PIXELS_PER_FRAME: f32 = 64.0;

/// Tightest zoom. At 24 fps this is about 12 px per minute, so an hour of
/// timeline fits in a typical panel.
pub const MIN_PIXELS_PER_FRAME: f32 = 0.008;

/// How ruler ticks are spaced, in sequence frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RulerStep {
    pub minor: i64,
    pub major: i64,
    pub label: i64,
}

pub fn clamp_timeline_zoom(pixels_per_frame: f32) -> f32 {
    if !pixels_per_frame.is_finite() || pixels_per_frame <= 0.0 {
        return 4.0;
    }
    pixels_per_frame.clamp(MIN_PIXELS_PER_FRAME, MAX_PIXELS_PER_FRAME)
}

/// Readout for the transport. Frame zoom names frames; pulled-back zoom names
/// seconds or minutes so a long cut is legible.
pub fn timeline_scale_label(pixels_per_frame: f32, fps: f64) -> String {
    let ppf = clamp_timeline_zoom(pixels_per_frame);
    let fps = if fps.is_finite() && fps > 1.0 {
        fps
    } else {
        24.0
    };
    if ppf >= 6.0 {
        return format!("{ppf:.1} px/frame");
    }
    let px_per_sec = ppf * fps as f32;
    if px_per_sec >= 6.0 {
        format!("{px_per_sec:.0} px/s")
    } else {
        format!("{:.0} px/min", px_per_sec * 60.0)
    }
}

/// Choose tick spacing so minor marks stay at least ~5 px apart and labels
/// stay at least ~72 px apart, from single frames out to hours.
pub fn ruler_step(pixels_per_frame: f32, fps: i64) -> RulerStep {
    let fps = fps.max(1);
    let ppf = if pixels_per_frame.is_finite() && pixels_per_frame > 0.0 {
        pixels_per_frame
    } else {
        1.0
    };
    let mut steps = vec![
        1,
        fps / 2,
        fps,
        fps * 2,
        fps * 5,
        fps * 10,
        fps * 15,
        fps * 30,
        fps * 60,
        fps * 120,
        fps * 300,
        fps * 600,
        fps * 1800,
        fps * 3600,
    ];
    steps.retain(|step| *step >= 1);
    steps.sort_unstable();
    steps.dedup();
    let pick = |min_px: f32, floor: i64| {
        steps
            .iter()
            .copied()
            .find(|step| *step >= floor && (*step as f32) * ppf >= min_px)
            .unwrap_or_else(|| steps[steps.len() - 1].max(floor))
    };
    let minor = pick(5.0, 1);
    let major = pick(14.0, minor);
    let label = pick(72.0, major);
    RulerStep {
        minor,
        major,
        label,
    }
}

/// First tick at or after `frame`, snapped down onto `step`.
pub fn align_frame(frame: i64, step: i64) -> i64 {
    let step = step.max(1);
    if frame <= 0 {
        0
    } else {
        frame - frame.rem_euclid(step)
    }
}

/// How many minor ticks fall in `start..=end`. Used to prove a long sequence
/// does not ask the ruler to draw a mark per frame.
pub fn ruler_mark_count(start: i64, end: i64, minor: i64) -> usize {
    if end < start {
        return 0;
    }
    let minor = minor.max(1);
    let mut frame = align_frame(start.max(0), minor);
    if frame < start {
        frame += minor;
    }
    if frame > end {
        return 0;
    }
    let span = (end - frame) as usize;
    span / (minor as usize) + 1
}

/// Index range of `clips` that intersect `[start, end)`.
///
/// `clips` must be sorted by `timeline_in`, which [`crate::Project::normalize`]
/// does. The range is exact when clips on the track do not overlap, which is
/// what overwrite and insert maintain. A later clip that ends at or before
/// `start` stops the backward walk.
pub fn visible_clip_span(clips: &[Clip], start: i64, end: i64) -> std::ops::Range<usize> {
    visible_span(
        clips,
        start,
        end,
        |clip| clip.timeline_in.0,
        |clip| clip.timeline_out.0,
    )
}

pub fn visible_span<T>(
    items: &[T],
    start: i64,
    end: i64,
    timeline_in: impl Fn(&T) -> i64,
    timeline_out: impl Fn(&T) -> i64,
) -> std::ops::Range<usize> {
    if items.is_empty() || end <= start {
        return 0..0;
    }
    let hi = items.partition_point(|item| timeline_in(item) < end);
    let mut lo = hi;
    while lo > 0 {
        if timeline_out(&items[lo - 1]) <= start {
            break;
        }
        lo -= 1;
    }
    lo..hi
}

/// Clip under `frame`, preferring the later start when two clips share an edge.
///
/// This is a binary search. It matches a reverse linear scan for a
/// non-overlapping, `timeline_in`-sorted track.
pub fn clip_index_at(clips: &[Clip], frame: i64) -> Option<usize> {
    if clips.is_empty() {
        return None;
    }
    let idx = clips.partition_point(|clip| clip.timeline_in.0 <= frame);
    if idx == 0 {
        return None;
    }
    let index = idx - 1;
    if clips[index].covers(Frame(frame)) {
        Some(index)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall(count: usize, length: i64) -> Vec<Clip> {
        (0..count)
            .map(|index| {
                let start = index as i64 * length;
                Clip::basic(index as u64 + 1, start, start + length)
            })
            .collect()
    }

    #[test]
    fn zoom_reaches_frames_and_minutes() {
        assert_eq!(clamp_timeline_zoom(0.0), 4.0);
        assert_eq!(clamp_timeline_zoom(f32::NAN), 4.0);
        assert_eq!(
            clamp_timeline_zoom(MIN_PIXELS_PER_FRAME / 10.0),
            MIN_PIXELS_PER_FRAME
        );
        assert_eq!(clamp_timeline_zoom(10_000.0), MAX_PIXELS_PER_FRAME);
        let hour = (24 * 60 * 60) as f32;
        let fitted = hour * MIN_PIXELS_PER_FRAME;
        assert!(
            fitted < 1_200.0,
            "an hour at the minimum zoom should fit a panel, got {fitted} px"
        );
        assert!(MAX_PIXELS_PER_FRAME >= 32.0);
        let label = timeline_scale_label(MIN_PIXELS_PER_FRAME, 24.0);
        assert!(label.contains("px/min"), "{label}");
        let frames = timeline_scale_label(16.0, 24.0);
        assert!(frames.contains("px/frame"), "{frames}");
    }

    #[test]
    fn minute_zoom_does_not_tick_every_frame_of_an_hour() {
        let hour = 24 * 60 * 60;
        let step = ruler_step(MIN_PIXELS_PER_FRAME, 24);
        assert!(step.minor > 1);
        assert!(step.label >= step.major && step.major >= step.minor);
        let marks = ruler_mark_count(0, hour, step.minor);
        assert!(
            marks < 400,
            "hour of timeline drew {marks} ticks at step {}",
            step.minor
        );
        let close = ruler_step(32.0, 24);
        let viewport_frames = (2_000.0 / 32.0) as i64;
        let close_marks = ruler_mark_count(50_000, 50_000 + viewport_frames, close.minor);
        assert!(close_marks < 500, "frame zoom drew {close_marks} ticks");
        assert_eq!(align_frame(0, 24), 0);
        assert_eq!(align_frame(25, 24), 24);
    }

    #[test]
    fn dense_sequence_culls_and_hit_tests_without_a_full_scan() {
        let clips = wall(800, 48);
        let window = visible_clip_span(&clips, 12_000, 12_240);
        assert!(!window.is_empty());
        assert!(
            window.len() <= 8,
            "viewport should not include the whole wall, got {}",
            window.len()
        );
        assert!(clips[window.clone()]
            .iter()
            .any(|clip| clip.covers(Frame(12_000))));
        assert!(clips[..window.start]
            .iter()
            .all(|clip| clip.timeline_out.0 <= 12_000));

        let long = Clip::basic(9, 0, 50_000);
        let span = visible_clip_span(&[long.clone()], 40_000, 40_100);
        assert_eq!(span, 0..1);
        assert_eq!(clip_index_at(&[long], 40_050), Some(0));

        for frame in (0..800 * 48).step_by(113) {
            let fast = clip_index_at(&clips, frame);
            let slow = clips.iter().rposition(|clip| clip.covers(Frame(frame)));
            assert_eq!(fast, slow, "frame {frame}");
        }
        assert_eq!(clip_index_at(&clips, -4), None);
        assert_eq!(visible_clip_span(&clips, 10, 10), 0..0);
    }
}
