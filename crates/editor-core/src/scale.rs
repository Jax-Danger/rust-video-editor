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
/// The latest clip that starts at or before `frame` is a binary search. On a
/// normal cut that clip is the only candidate. `stacked` lists clips that end
/// after a later clip (a title or another angle on the same track); those are
/// checked only when the top candidate does not cover `frame`.
pub fn clip_index_at(clips: &[Clip], frame: i64, stacked: &[u32]) -> Option<usize> {
    if clips.is_empty() {
        return None;
    }
    let idx = clips.partition_point(|clip| clip.timeline_in.0 <= frame);
    if idx > 0 && clips[idx - 1].covers(Frame(frame)) {
        return Some(idx - 1);
    }
    let mut best = None;
    for &raw in stacked {
        let index = raw as usize;
        if index < clips.len() && clips[index].covers(Frame(frame)) {
            best = Some(index);
        }
    }
    best
}

/// Clips that end after some later clip on the same track.
///
/// A non-overlapping cut returns an empty list, so drawing stays a binary
/// search. The list is short: one entry per clip that sticks out past a shot
/// placed after it.
pub fn stacked_clip_indices(clips: &[Clip]) -> Vec<u32> {
    let mut min_after = i64::MAX;
    let mut stacked = Vec::new();
    for (index, clip) in clips.iter().enumerate().rev() {
        if clip.timeline_out.0 > min_after {
            stacked.push(index as u32);
        }
        min_after = min_after.min(clip.timeline_out.0);
    }
    stacked.reverse();
    stacked
}

/// Stacked clips that intersect `[start, end)` and start before `span_start`.
///
/// [`visible_clip_span`] stops at the first later clip that has already ended,
/// which is exact when nothing underneath runs longer. These indices are the
/// underneath clips the search does not return.
pub fn stacked_hits(
    clips: &[Clip],
    stacked: &[u32],
    start: i64,
    end: i64,
    span_start: usize,
) -> Vec<usize> {
    let mut hits = Vec::new();
    for &raw in stacked {
        let index = raw as usize;
        if index >= span_start || index >= clips.len() {
            continue;
        }
        let clip = &clips[index];
        if clip.timeline_in.0 < end && clip.timeline_out.0 > start {
            hits.push(index);
        }
    }
    hits
}

/// Pixel X of `frame` in a viewport whose left edge is `origin_frame`.
///
/// The frame delta is computed in f64. An hour-long sequence zoomed to single
/// frames stays a viewport coordinate instead of a multi-million-pixel strip.
pub fn timeline_x(frame: i64, origin_frame: f64, view_left: f32, pixels_per_frame: f32) -> f32 {
    let ppf = if pixels_per_frame.is_finite() && pixels_per_frame > 0.0 {
        pixels_per_frame as f64
    } else {
        1.0
    };
    let origin = if origin_frame.is_finite() {
        origin_frame
    } else {
        0.0
    };
    (view_left as f64 + (frame as f64 - origin) * ppf) as f32
}

/// Scroll origin after a zoom that keeps `anchor_px` on the same frame.
///
/// The anchor frame is clamped to the sequence, and a zoom that still fits the
/// whole sequence pins the origin at 0. That stops an overview from jumping to
/// the tail when the panel center sits in the empty space after the last clip.
pub fn zoom_origin(
    origin: f64,
    old_ppf: f32,
    new_ppf: f32,
    anchor_px: f32,
    sequence_end: i64,
    view_width: f32,
) -> f64 {
    let new = if new_ppf.is_finite() && new_ppf > 0.0 {
        new_ppf
    } else {
        1.0
    };
    let old = if old_ppf.is_finite() && old_ppf > 0.0 {
        old_ppf
    } else {
        new
    };
    let anchor = anchor_px.max(0.0) as f64;
    let origin = if origin.is_finite() { origin } else { 0.0 };
    let end = sequence_end.max(0) as f64;
    let frame = (origin + anchor / f64::from(old)).clamp(0.0, end);
    let width = if view_width.is_finite() {
        view_width.max(1.0)
    } else {
        1.0
    };
    let view_frames = f64::from(width) / f64::from(new);
    if view_frames + 1.0 >= end {
        return 0.0;
    }
    let max_origin = (end - view_frames).max(0.0);
    (frame - anchor / f64::from(new)).clamp(0.0, max_origin)
}

/// Frame under a viewport pixel. Inverse of [`timeline_x`].
pub fn frame_at_x(x: f32, origin_frame: f64, view_left: f32, pixels_per_frame: f32) -> i64 {
    let ppf = if pixels_per_frame.is_finite() && pixels_per_frame > 0.0 {
        pixels_per_frame as f64
    } else {
        1.0
    };
    let origin = if origin_frame.is_finite() {
        origin_frame
    } else {
        0.0
    };
    (origin + (x - view_left) as f64 / ppf).round() as i64
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
        assert_eq!(clip_index_at(&[long], 40_050, &[]), Some(0));

        for frame in (0..800 * 48).step_by(113) {
            let fast = clip_index_at(&clips, frame, &[]);
            let slow = clips.iter().rposition(|clip| clip.covers(Frame(frame)));
            assert_eq!(fast, slow, "frame {frame}");
        }
        assert_eq!(clip_index_at(&clips, -4, &[]), None);
        assert_eq!(visible_clip_span(&clips, 10, 10), 0..0);
    }

    #[test]
    fn stacked_clip_is_drawn_and_hit_under_later_shots() {
        let clips = vec![
            Clip::basic(1, 0, 50_000),
            Clip::basic(2, 0, 100),
            Clip::basic(3, 20_000, 20_048),
        ];
        let stacked = stacked_clip_indices(&clips);
        assert_eq!(stacked, vec![0]);
        let span = visible_clip_span(&clips, 5_000, 5_100);
        assert!(
            !clips[span.clone()].iter().any(|clip| clip.id.0 == 1),
            "the binary search window is the later shots, not the long clip"
        );
        let hits = stacked_hits(&clips, &stacked, 5_000, 5_100, span.start);
        assert_eq!(hits, vec![0]);
        assert_eq!(clip_index_at(&clips, 5_050, &stacked), Some(0));
        assert_eq!(
            clip_index_at(&clips, 50, &stacked).map(|i| clips[i].id.0),
            Some(2)
        );
        assert!(stacked_clip_indices(&wall(40, 24)).is_empty());
    }

    #[test]
    fn frame_zoom_an_hour_in_stays_inside_the_viewport() {
        let hour = 24 * 60 * 60;
        let origin = hour as f64 - 12.0;
        let x = timeline_x(hour, origin, 80.0, 64.0);
        assert!((x - (80.0 + 12.0 * 64.0)).abs() < 0.5, "viewport x {x}");
        assert!(x < 2_000.0, "hour at frame zoom landed at {x}px");
        assert_eq!(frame_at_x(x, origin, 80.0, 64.0), hour);
        let fitted = timeline_x(hour, 0.0, 0.0, MIN_PIXELS_PER_FRAME);
        assert!(fitted < 1_200.0, "overview of an hour is {fitted}px");
    }

    #[test]
    fn zoom_in_from_an_overview_does_not_jump_to_the_tail() {
        let end = 400 * 48;
        let width = 1_600.0;
        let stayed = zoom_origin(0.0, 0.05, 4.0, 0.0, end, width);
        assert!(
            stayed < 50.0,
            "playhead at frame 0 should stay at the start, origin {stayed}"
        );
        let middle = zoom_origin(0.0, 0.05, 4.0, width * 0.5, end, width);
        let anchor_frame = (width * 0.5) as f64 / 0.05;
        assert!(
            (middle - (anchor_frame.min(end as f64) - (width * 0.5) as f64 / 4.0)).abs() < 2.0,
            "origin {middle}"
        );
        assert!(middle < end as f64);
        let fitted = zoom_origin(8_000.0, 4.0, width / end as f32, 100.0, end, width);
        assert!(
            fitted.abs() < 1.0,
            "a zoom that fits the sequence pins to the start, origin {fitted}"
        );
    }
}
