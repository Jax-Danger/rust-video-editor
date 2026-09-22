//! Timeline edits.
//!
//! Ripple trim changes sequence duration without opening a gap. `delta > 0`
//! lengthens the clip. A head ripple keeps `timeline_in` parked and reveals or
//! hides source at the head; a tail ripple moves `timeline_out`. Downstream
//! sync-locked tracks shift with the edit point. Roll, slip, and slide do not
//! change the outer span of the clips they touch.
//!
//! Regular trim moves one edge and stops at the neighbouring clip.

use thiserror::Error;

use crate::caption::CaptionDraft;
use crate::effects::{color_grade_mut, transform_mut, GradeParam, TransformParam};
use crate::model::{
    CaptionCue, Clip, ClipId, CueId, Marker, MarkerId, MediaAsset, MediaId, Project, Sequence,
    SequenceId, TrackId, TrackKind, Transition, TransitionAlign, TransitionId, TransitionKind,
};
use crate::time::{convert_frames, mul_div_round, Frame, Timebase};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EditError {
    #[error("no active sequence")]
    NoActiveSequence,
    #[error("sequence not found")]
    SequenceNotFound,
    #[error("track not found")]
    TrackNotFound,
    #[error("clip not found")]
    ClipNotFound,
    #[error("media not found")]
    MediaNotFound,
    #[error("track is locked")]
    TrackLocked,
    #[error("wrong track kind")]
    WrongTrackKind,
    #[error("clip duration must be at least one frame")]
    InvalidDuration,
    #[error("edit is outside the timeline")]
    OutOfRange,
    #[error("playhead is not inside a clip")]
    NotInsideClip,
    #[error("clips are not adjacent")]
    NotAdjacent,
    #[error("not enough media handle (have {have}, need {need})")]
    InsufficientHandle { have: i64, need: i64 },
    #[error("transition does not fit on the cut")]
    TransitionDoesNotFit,
    #[error("caption track missing")]
    NoCaptionTrack,
    #[error("clips must share an in-point and duration")]
    MismatchedSpan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrimEdge {
    Head,
    Tail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    ClipEdge,
    Playhead,
    Marker,
    InPoint,
    OutPoint,
    Origin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapPoint {
    pub frame: Frame,
    pub kind: SnapKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapHit {
    pub frame: Frame,
    pub distance: i64,
    pub kind: SnapKind,
}

pub fn snap_to_targets(frame: Frame, targets: &[SnapPoint], threshold: i64) -> Option<SnapHit> {
    let mut best: Option<SnapHit> = None;
    for target in targets {
        let distance = (target.frame.0 - frame.0).abs();
        if distance > threshold {
            continue;
        }
        let replace = match best {
            None => true,
            Some(current) => distance < current.distance,
        };
        if replace {
            best = Some(SnapHit {
                frame: target.frame,
                distance,
                kind: target.kind,
            });
        }
    }
    best
}

pub fn collect_snap_points(
    sequence: &Sequence,
    exclude: &[ClipId],
    playhead: Option<Frame>,
) -> Vec<SnapPoint> {
    let mut points = vec![SnapPoint {
        frame: Frame::ZERO,
        kind: SnapKind::Origin,
    }];
    if let Some(playhead) = playhead {
        points.push(SnapPoint {
            frame: playhead,
            kind: SnapKind::Playhead,
        });
    }
    if let Some(frame) = sequence.in_point {
        points.push(SnapPoint {
            frame,
            kind: SnapKind::InPoint,
        });
    }
    if let Some(frame) = sequence.out_point {
        points.push(SnapPoint {
            frame,
            kind: SnapKind::OutPoint,
        });
    }
    for marker in &sequence.markers {
        points.push(SnapPoint {
            frame: marker.frame,
            kind: SnapKind::Marker,
        });
    }
    for track in &sequence.tracks {
        for clip in &track.clips {
            if exclude.contains(&clip.id) {
                continue;
            }
            points.push(SnapPoint {
                frame: clip.timeline_in,
                kind: SnapKind::ClipEdge,
            });
            points.push(SnapPoint {
                frame: clip.timeline_out,
                kind: SnapKind::ClipEdge,
            });
        }
    }
    points
}

/// Snap a moved clip by whichever of its edges lands closer to a target.
pub fn snap_span(start: Frame, duration: i64, targets: &[SnapPoint], threshold: i64) -> Frame {
    let in_hit = snap_to_targets(start, targets, threshold);
    let out_hit =
        snap_to_targets(Frame(start.0 + duration), targets, threshold).map(|hit| SnapHit {
            frame: Frame(hit.frame.0 - duration),
            distance: hit.distance,
            kind: hit.kind,
        });
    match (in_hit, out_hit) {
        (Some(a), Some(b)) => {
            if a.distance <= b.distance {
                a.frame
            } else {
                b.frame
            }
        }
        (Some(a), None) => a.frame,
        (None, Some(b)) => b.frame,
        (None, None) => start,
    }
}

fn map_sequence(
    project: &mut Project,
    sequence_id: SequenceId,
    f: impl FnOnce(&mut Sequence, &mut dyn FnMut() -> u64) -> Result<(), EditError>,
) -> Result<(), EditError> {
    let index = project
        .sequences
        .iter()
        .position(|s| s.id == sequence_id)
        .ok_or(EditError::SequenceNotFound)?;
    let mut sequence = project.sequences[index].clone();
    let mut next = project.next_id;
    let mut alloc = || {
        let id = next;
        next = next.saturating_add(1);
        id
    };
    let result = f(&mut sequence, &mut alloc);
    if result.is_ok() {
        project.next_id = next;
        project.sequences[index] = sequence;
    }
    result
}

fn active_id(project: &Project) -> Result<SequenceId, EditError> {
    project.active_sequence.ok_or(EditError::NoActiveSequence)
}

pub fn overwrite_clips(
    project: &mut Project,
    sequence_id: SequenceId,
    incoming: Vec<(TrackId, Clip)>,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, alloc| {
        for (track_id, clip) in incoming {
            place_overwrite(sequence, track_id, clip, alloc)?;
        }
        cleanup_transitions(sequence);
        Ok(())
    })
}

pub fn insert_clips(
    project: &mut Project,
    sequence_id: SequenceId,
    incoming: Vec<(TrackId, Clip)>,
) -> Result<(), EditError> {
    if incoming.is_empty() {
        return Ok(());
    }
    let start = incoming[0].1.timeline_in;
    let duration = incoming[0].1.duration();
    if duration <= 0 {
        return Err(EditError::InvalidDuration);
    }
    if incoming
        .iter()
        .any(|(_, clip)| clip.timeline_in != start || clip.duration() != duration)
    {
        return Err(EditError::MismatchedSpan);
    }
    map_sequence(project, sequence_id, |sequence, alloc| {
        for (track_id, _) in &incoming {
            let track = sequence.track(*track_id).ok_or(EditError::TrackNotFound)?;
            if track.locked {
                return Err(EditError::TrackLocked);
            }
        }
        let force: Vec<TrackId> = incoming.iter().map(|(track, _)| *track).collect();
        split_and_shift(sequence, start.0, duration, &force, alloc)?;
        for (track_id, clip) in incoming {
            let track = sequence
                .tracks
                .iter_mut()
                .find(|t| t.id == track_id)
                .ok_or(EditError::TrackNotFound)?;
            track.clips.push(clip);
            track.clips.sort_by_key(|c| (c.timeline_in.0, c.id.0));
        }
        cleanup_transitions(sequence);
        Ok(())
    })
}

pub fn razor_clip(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    at: Frame,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, alloc| {
        let primary = sequence
            .clip(clip_id)
            .ok_or(EditError::ClipNotFound)?
            .clone();
        if !primary.contains_frame(at) {
            return Err(EditError::NotInsideClip);
        }
        let mut targets = vec![clip_id];
        for linked in &primary.linked {
            if let Some(partner) = sequence.clip(*linked) {
                if partner.contains_frame(at) {
                    targets.push(*linked);
                }
            }
        }
        targets.sort();
        targets.dedup();
        let mut pairs = Vec::new();
        for id in targets {
            let right = split_clip(sequence, id, at, alloc)?;
            pairs.push((id, right));
        }
        relink_splits(sequence, &pairs);
        Ok(())
    })
}

/// Split every unlocked clip that contains `at`.
pub fn razor_at(
    project: &mut Project,
    sequence_id: SequenceId,
    at: Frame,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, alloc| {
        let mut targets = Vec::new();
        for track in &sequence.tracks {
            if track.locked {
                continue;
            }
            for clip in &track.clips {
                if clip.contains_frame(at) {
                    targets.push(clip.id);
                }
            }
        }
        if targets.is_empty() {
            return Err(EditError::NotInsideClip);
        }
        let mut pairs = Vec::new();
        for id in targets {
            let right = split_clip(sequence, id, at, alloc)?;
            pairs.push((id, right));
        }
        relink_splits(sequence, &pairs);
        Ok(())
    })
}

pub fn lift_delete(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_ids: &[ClipId],
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        ensure_unlocked(sequence, clip_ids)?;
        remove_clips(sequence, clip_ids);
        cleanup_transitions(sequence);
        Ok(())
    })
}

pub fn ripple_delete(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_ids: &[ClipId],
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        ensure_unlocked(sequence, clip_ids)?;
        let mut ranges: Vec<(usize, i64, i64)> = Vec::new();
        for id in clip_ids {
            if let Some((ti, ci)) = sequence.locate_clip(*id) {
                let clip = &sequence.tracks[ti].clips[ci];
                ranges.push((ti, clip.timeline_in.0, clip.timeline_out.0));
            }
        }
        remove_clips(sequence, clip_ids);
        let mut tracks: Vec<usize> = ranges.iter().map(|(ti, _, _)| *ti).collect();
        tracks.sort_unstable();
        tracks.dedup();
        for ti in tracks {
            let mut gaps: Vec<(i64, i64)> = ranges
                .iter()
                .filter(|(track, _, _)| *track == ti)
                .map(|(_, start, end)| (*start, *end))
                .collect();
            gaps.sort_by_key(|(start, _)| *start);
            gaps.reverse();
            for (start, end) in gaps {
                let dur = end - start;
                if dur <= 0 {
                    continue;
                }
                for clip in &mut sequence.tracks[ti].clips {
                    if clip.timeline_in.0 >= end {
                        clip.timeline_in.0 -= dur;
                        clip.timeline_out.0 -= dur;
                    }
                }
                for cue in &mut sequence.tracks[ti].cues {
                    if cue.timeline_in.0 >= end {
                        cue.timeline_in.0 -= dur;
                        cue.timeline_out.0 -= dur;
                    }
                }
            }
            sequence.tracks[ti]
                .clips
                .sort_by_key(|c| (c.timeline_in.0, c.id.0));
        }
        cleanup_transitions(sequence);
        Ok(())
    })
}

pub fn move_clips(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_ids: &[ClipId],
    delta: i64,
    retarget: Option<(ClipId, TrackId)>,
) -> Result<(), EditError> {
    if delta == 0 && retarget.is_none() {
        return Ok(());
    }
    map_sequence(project, sequence_id, |sequence, alloc| {
        ensure_unlocked(sequence, clip_ids)?;
        if let Some((_, track_id)) = retarget {
            let track = sequence.track(track_id).ok_or(EditError::TrackNotFound)?;
            if track.locked {
                return Err(EditError::TrackLocked);
            }
        }
        let mut taken: Vec<(TrackId, Clip)> = Vec::new();
        for id in clip_ids {
            if sequence.locate_clip(*id).is_none() {
                continue;
            }
            taken.push(take_clip(sequence, *id)?);
        }
        for (mut track_id, mut clip) in taken {
            if let Some((retarget_clip, new_track)) = retarget {
                if clip.id == retarget_clip {
                    let from_kind = sequence
                        .track(track_id)
                        .map(|t| t.kind)
                        .ok_or(EditError::TrackNotFound)?;
                    let to_kind = sequence
                        .track(new_track)
                        .map(|t| t.kind)
                        .ok_or(EditError::TrackNotFound)?;
                    if from_kind != to_kind {
                        return Err(EditError::WrongTrackKind);
                    }
                    track_id = new_track;
                }
            }
            clip.timeline_in.0 += delta;
            clip.timeline_out.0 += delta;
            if clip.timeline_in.0 < 0 {
                return Err(EditError::OutOfRange);
            }
            place_overwrite(sequence, track_id, clip, alloc)?;
        }
        cleanup_transitions(sequence);
        Ok(())
    })
}

/// Move one edge. Positive `delta` moves the edge to the right.
/// The edit stops with [`EditError::OutOfRange`] if it would overlap a neighbour
/// or leave the media limits; callers that want a nudge should pass a legal delta.
pub fn trim(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    edge: TrimEdge,
    delta: i64,
) -> Result<(), EditError> {
    if delta == 0 {
        return Ok(());
    }
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let timebase = sequence.timebase;
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        if sequence.tracks[ti].locked {
            return Err(EditError::TrackLocked);
        }
        let clips = &sequence.tracks[ti].clips;
        match edge {
            TrimEdge::Head => {
                let new_in = clips[ci].timeline_in.0 + delta;
                let new_out = clips[ci].timeline_out.0;
                if new_out - new_in < 1 {
                    return Err(EditError::InvalidDuration);
                }
                if let Some(prev) = clips.iter().take(ci).filter(|c| c.id != clip_id).last() {
                    if new_in < prev.timeline_out.0 {
                        return Err(EditError::OutOfRange);
                    }
                }
                if new_in < 0 {
                    return Err(EditError::OutOfRange);
                }
                let src = source_delta(&clips[ci], delta, timebase);
                let new_source = clips[ci].source_in.0 + src;
                if new_source < clips[ci].source_min.0 || new_source >= clips[ci].source_out.0 {
                    return Err(EditError::InsufficientHandle {
                        have: clips[ci].head_handle(),
                        need: src.abs(),
                    });
                }
                let clip = &mut sequence.tracks[ti].clips[ci];
                clip.timeline_in = Frame(new_in);
                clip.source_in = Frame(new_source);
            }
            TrimEdge::Tail => {
                let new_out = clips[ci].timeline_out.0 + delta;
                let new_in = clips[ci].timeline_in.0;
                if new_out - new_in < 1 {
                    return Err(EditError::InvalidDuration);
                }
                if let Some(next) = clips.iter().skip(ci + 1).find(|c| c.id != clip_id) {
                    if new_out > next.timeline_in.0 {
                        return Err(EditError::OutOfRange);
                    }
                }
                let src = source_delta(&clips[ci], delta, timebase);
                let new_source = clips[ci].source_out.0 + src;
                if new_source > clips[ci].source_max.0 || new_source <= clips[ci].source_in.0 {
                    return Err(EditError::InsufficientHandle {
                        have: clips[ci].tail_handle(),
                        need: src.abs(),
                    });
                }
                let clip = &mut sequence.tracks[ti].clips[ci];
                clip.timeline_out = Frame(new_out);
                clip.source_out = Frame(new_source);
            }
        }
        Ok(())
    })
}

/// `delta > 0` lengthens the clip. See module docs for head vs tail.
pub fn ripple_trim(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    edge: TrimEdge,
    delta: i64,
) -> Result<(), EditError> {
    if delta == 0 {
        return Ok(());
    }
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let timebase = sequence.timebase;
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        let origin = sequence.tracks[ti].id;
        if sequence.tracks[ti].locked {
            return Err(EditError::TrackLocked);
        }
        let clip = sequence.tracks[ti].clips[ci].clone();
        let old_out = clip.timeline_out.0;
        let src = source_delta(&clip, delta, timebase);
        match edge {
            TrimEdge::Tail => {
                let new_out = clip.timeline_out.0 + delta;
                let new_source = clip.source_out.0 + src;
                if new_out - clip.timeline_in.0 < 1 || new_source <= clip.source_in.0 {
                    return Err(EditError::InvalidDuration);
                }
                if new_source > clip.source_max.0 || new_source < clip.source_min.0 {
                    return Err(EditError::InsufficientHandle {
                        have: clip.tail_handle(),
                        need: src.abs(),
                    });
                }
                sequence.tracks[ti].clips[ci].timeline_out = Frame(new_out);
                sequence.tracks[ti].clips[ci].source_out = Frame(new_source);
            }
            TrimEdge::Head => {
                let new_source = clip.source_in.0 - src;
                let new_out = clip.timeline_out.0 + delta;
                if new_out - clip.timeline_in.0 < 1 {
                    return Err(EditError::InvalidDuration);
                }
                if new_source < clip.source_min.0 || new_source >= clip.source_out.0 {
                    return Err(EditError::InsufficientHandle {
                        have: clip.head_handle(),
                        need: src.abs(),
                    });
                }
                // Head ripple keeps the in-point parked. Earlier or later source
                // is revealed there, and the tail plus everything downstream moves.
                sequence.tracks[ti].clips[ci].source_in = Frame(new_source);
                sequence.tracks[ti].clips[ci].timeline_out = Frame(new_out);
            }
        }
        ripple_downstream(sequence, origin, old_out, delta, clip_id)?;
        Ok(())
    })
}

/// Positive `delta` moves the cut later. `left_clip` is the clip before the cut.
pub fn roll_cut(
    project: &mut Project,
    sequence_id: SequenceId,
    left_clip: ClipId,
    delta: i64,
) -> Result<(), EditError> {
    if delta == 0 {
        return Ok(());
    }
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let timebase = sequence.timebase;
        let (ti, ci) = sequence
            .locate_clip(left_clip)
            .ok_or(EditError::ClipNotFound)?;
        if sequence.tracks[ti].locked {
            return Err(EditError::TrackLocked);
        }
        let right_index = ci + 1;
        if right_index >= sequence.tracks[ti].clips.len() {
            return Err(EditError::NotAdjacent);
        }
        let left = sequence.tracks[ti].clips[ci].clone();
        let right = sequence.tracks[ti].clips[right_index].clone();
        if left.timeline_out != right.timeline_in {
            return Err(EditError::NotAdjacent);
        }
        if left.duration() + delta < 1 || right.duration() - delta < 1 {
            return Err(EditError::InvalidDuration);
        }
        let src_left = source_delta(&left, delta, timebase);
        let src_right = source_delta(&right, delta, timebase);
        let new_left_source = left.source_out.0 + src_left;
        let new_right_source = right.source_in.0 + src_right;
        if new_left_source > left.source_max.0 || new_left_source <= left.source_in.0 {
            return Err(EditError::InsufficientHandle {
                have: left.tail_handle(),
                need: src_left.abs(),
            });
        }
        if new_right_source < right.source_min.0 || new_right_source >= right.source_out.0 {
            return Err(EditError::InsufficientHandle {
                have: right.head_handle(),
                need: src_right.abs(),
            });
        }
        sequence.tracks[ti].clips[ci].timeline_out = Frame(left.timeline_out.0 + delta);
        sequence.tracks[ti].clips[ci].source_out = Frame(new_left_source);
        sequence.tracks[ti].clips[right_index].timeline_in = Frame(right.timeline_in.0 + delta);
        sequence.tracks[ti].clips[right_index].source_in = Frame(new_right_source);
        Ok(())
    })
}

/// Positive `delta` shows later source. Timeline position does not change.
pub fn slip(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    delta: i64,
) -> Result<(), EditError> {
    if delta == 0 {
        return Ok(());
    }
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let timebase = sequence.timebase;
        let ids = linked_group(sequence, clip_id);
        for id in ids {
            let (ti, ci) = sequence.locate_clip(id).ok_or(EditError::ClipNotFound)?;
            if sequence.tracks[ti].locked {
                return Err(EditError::TrackLocked);
            }
            let clip = sequence.tracks[ti].clips[ci].clone();
            let src = source_delta(&clip, delta, timebase);
            let new_in = clip.source_in.0 + src;
            let new_out = clip.source_out.0 + src;
            if new_in < clip.source_min.0 || new_out > clip.source_max.0 {
                let have = if src > 0 {
                    clip.tail_handle()
                } else {
                    clip.head_handle()
                };
                return Err(EditError::InsufficientHandle {
                    have,
                    need: src.abs(),
                });
            }
            sequence.tracks[ti].clips[ci].source_in = Frame(new_in);
            sequence.tracks[ti].clips[ci].source_out = Frame(new_out);
        }
        Ok(())
    })
}

/// Positive `delta` slides the clip later. The previous clip's tail and the
/// next clip's head absorb the move. The outer span stays put.
pub fn slide(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    delta: i64,
) -> Result<(), EditError> {
    if delta == 0 {
        return Ok(());
    }
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let timebase = sequence.timebase;
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        if sequence.tracks[ti].locked {
            return Err(EditError::TrackLocked);
        }
        if ci == 0 || ci + 1 >= sequence.tracks[ti].clips.len() {
            return Err(EditError::NotAdjacent);
        }
        let prev = sequence.tracks[ti].clips[ci - 1].clone();
        let clip = sequence.tracks[ti].clips[ci].clone();
        let next = sequence.tracks[ti].clips[ci + 1].clone();
        if prev.timeline_out != clip.timeline_in || clip.timeline_out != next.timeline_in {
            return Err(EditError::NotAdjacent);
        }
        if prev.duration() + delta < 1 || next.duration() - delta < 1 {
            return Err(EditError::InvalidDuration);
        }
        if clip.timeline_in.0 + delta < prev.timeline_in.0 {
            return Err(EditError::InvalidDuration);
        }
        let src_prev = source_delta(&prev, delta, timebase);
        let src_next = source_delta(&next, delta, timebase);
        let new_prev_source = prev.source_out.0 + src_prev;
        let new_next_source = next.source_in.0 + src_next;
        if new_prev_source > prev.source_max.0 || new_prev_source <= prev.source_in.0 {
            return Err(EditError::InsufficientHandle {
                have: prev.tail_handle(),
                need: src_prev.abs(),
            });
        }
        if new_next_source < next.source_min.0 || new_next_source >= next.source_out.0 {
            return Err(EditError::InsufficientHandle {
                have: next.head_handle(),
                need: src_next.abs(),
            });
        }
        sequence.tracks[ti].clips[ci - 1].timeline_out = Frame(prev.timeline_out.0 + delta);
        sequence.tracks[ti].clips[ci - 1].source_out = Frame(new_prev_source);
        sequence.tracks[ti].clips[ci].timeline_in = Frame(clip.timeline_in.0 + delta);
        sequence.tracks[ti].clips[ci].timeline_out = Frame(clip.timeline_out.0 + delta);
        sequence.tracks[ti].clips[ci + 1].timeline_in = Frame(next.timeline_in.0 + delta);
        sequence.tracks[ti].clips[ci + 1].source_in = Frame(new_next_source);
        Ok(())
    })
}

pub fn add_transition(
    project: &mut Project,
    sequence_id: SequenceId,
    left_clip: ClipId,
    kind: TransitionKind,
    duration: i64,
    alignment: TransitionAlign,
) -> Result<TransitionId, EditError> {
    if duration < 1 {
        return Err(EditError::InvalidDuration);
    }
    let mut created = TransitionId(0);
    map_sequence(project, sequence_id, |sequence, alloc| {
        let (ti, ci) = sequence
            .locate_clip(left_clip)
            .ok_or(EditError::ClipNotFound)?;
        if sequence.tracks[ti].locked {
            return Err(EditError::TrackLocked);
        }
        if ci + 1 >= sequence.tracks[ti].clips.len() {
            return Err(EditError::NotAdjacent);
        }
        let left = sequence.tracks[ti].clips[ci].clone();
        let right = sequence.tracks[ti].clips[ci + 1].clone();
        if left.timeline_out != right.timeline_in {
            return Err(EditError::NotAdjacent);
        }
        let (left_overlap, right_overlap) = overlap_pair(duration, alignment);
        if left.duration() <= left_overlap || right.duration() < right_overlap {
            return Err(EditError::TransitionDoesNotFit);
        }
        let need_left = source_delta(&left, left_overlap, sequence.timebase).abs();
        let need_right = source_delta(&right, right_overlap, sequence.timebase).abs();
        if left.tail_handle() < need_left {
            return Err(EditError::InsufficientHandle {
                have: left.tail_handle(),
                need: need_left,
            });
        }
        if right.head_handle() < need_right {
            return Err(EditError::InsufficientHandle {
                have: right.head_handle(),
                need: need_right,
            });
        }
        sequence.tracks[ti].transitions.retain(|t| {
            t.left_clip != left.id && t.right_clip != right.id && t.left_clip != right.id
        });
        let id = TransitionId(alloc());
        created = id;
        sequence.tracks[ti].transitions.push(Transition {
            id,
            kind,
            left_clip: left.id,
            right_clip: right.id,
            duration,
            alignment,
        });
        Ok(())
    })?;
    Ok(created)
}

pub fn set_grade_at(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    param: GradeParam,
    clip_relative_frame: i64,
    value: f32,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        let grade = color_grade_mut(&mut sequence.tracks[ti].clips[ci].effects);
        grade.param_mut(param).write_at(clip_relative_frame, value);
        Ok(())
    })
}

pub fn set_transform_at(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    param: TransformParam,
    clip_relative_frame: i64,
    value: f32,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        let transform = transform_mut(&mut sequence.tracks[ti].clips[ci].effects);
        transform
            .param_mut(param)
            .write_at(clip_relative_frame, value);
        Ok(())
    })
}

pub fn set_clip_volume(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    volume: f32,
) -> Result<(), EditError> {
    set_clip_gain_at(project, sequence_id, clip_id, 0, volume)
}

/// Write clip gain at a clip-relative frame.
///
/// With no keys this edits the constant. With keys it writes a key, matching
/// the other animated parameters.
pub fn set_clip_gain_at(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    clip_relative_frame: i64,
    volume: f32,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        sequence.tracks[ti].clips[ci]
            .volume
            .write_at(clip_relative_frame, crate::mix::clamp_gain(volume));
        Ok(())
    })
}

pub fn toggle_volume_key(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    clip_relative_frame: i64,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        let anim = &mut sequence.tracks[ti].clips[ci].volume;
        if anim.has_key(clip_relative_frame) {
            let value = anim.value_at(clip_relative_frame);
            anim.remove_key(clip_relative_frame);
            if anim.keys.is_empty() {
                anim.base = value;
            }
        } else {
            let value = anim.value_at(clip_relative_frame);
            anim.set_key(clip_relative_frame, value);
        }
        Ok(())
    })
}

pub fn set_track_fader(
    project: &mut Project,
    sequence_id: SequenceId,
    track_id: TrackId,
    fader: f32,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let track = sequence
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or(EditError::TrackNotFound)?;
        track.fader = crate::mix::clamp_gain(fader);
        Ok(())
    })
}

pub fn set_track_pan(
    project: &mut Project,
    sequence_id: SequenceId,
    track_id: TrackId,
    pan: f32,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let track = sequence
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or(EditError::TrackNotFound)?;
        track.pan = crate::mix::clamp_pan(pan);
        Ok(())
    })
}

pub fn set_master_fader(
    project: &mut Project,
    sequence_id: SequenceId,
    fader: f32,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        sequence.master_fader = crate::mix::clamp_gain(fader);
        Ok(())
    })
}

pub fn toggle_grade_key(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    param: GradeParam,
    clip_relative_frame: i64,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        let grade = color_grade_mut(&mut sequence.tracks[ti].clips[ci].effects);
        let anim = grade.param_mut(param);
        if anim.has_key(clip_relative_frame) {
            let value = anim.value_at(clip_relative_frame);
            anim.remove_key(clip_relative_frame);
            if anim.keys.is_empty() {
                anim.base = value;
            }
        } else {
            let value = anim.value_at(clip_relative_frame);
            anim.set_key(clip_relative_frame, value);
        }
        Ok(())
    })
}

pub fn toggle_transform_key(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    param: TransformParam,
    clip_relative_frame: i64,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        let transform = transform_mut(&mut sequence.tracks[ti].clips[ci].effects);
        let anim = transform.param_mut(param);
        if anim.has_key(clip_relative_frame) {
            let value = anim.value_at(clip_relative_frame);
            anim.remove_key(clip_relative_frame);
            if anim.keys.is_empty() {
                anim.base = value;
            }
        } else {
            let value = anim.value_at(clip_relative_frame);
            anim.set_key(clip_relative_frame, value);
        }
        Ok(())
    })
}

pub fn replace_captions(
    project: &mut Project,
    sequence_id: SequenceId,
    track_id: TrackId,
    drafts: Vec<CaptionDraft>,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, alloc| {
        let track = sequence
            .tracks
            .iter_mut()
            .find(|t| t.id == track_id)
            .ok_or(EditError::TrackNotFound)?;
        if track.kind != TrackKind::Caption {
            return Err(EditError::WrongTrackKind);
        }
        if track.locked {
            return Err(EditError::TrackLocked);
        }
        track.cues.clear();
        for draft in drafts {
            if draft.timeline_out.0 <= draft.timeline_in.0 {
                continue;
            }
            track.cues.push(CaptionCue {
                id: CueId(alloc()),
                timeline_in: draft.timeline_in,
                timeline_out: draft.timeline_out,
                text: draft.text,
                speaker: draft.speaker,
            });
        }
        track.cues.sort_by_key(|c| c.timeline_in.0);
        Ok(())
    })
}

pub fn update_cue_text(
    project: &mut Project,
    sequence_id: SequenceId,
    cue_id: CueId,
    text: String,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        for track in &mut sequence.tracks {
            if let Some(cue) = track.cues.iter_mut().find(|c| c.id == cue_id) {
                cue.text = text;
                return Ok(());
            }
        }
        Err(EditError::ClipNotFound)
    })
}

pub fn delete_cue(
    project: &mut Project,
    sequence_id: SequenceId,
    cue_id: CueId,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        for track in &mut sequence.tracks {
            let before = track.cues.len();
            track.cues.retain(|c| c.id != cue_id);
            if track.cues.len() != before {
                return Ok(());
            }
        }
        Err(EditError::ClipNotFound)
    })
}

pub fn set_track_flag(
    project: &mut Project,
    sequence_id: SequenceId,
    track_id: TrackId,
    flag: TrackFlag,
    value: bool,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let track = sequence
            .tracks
            .iter_mut()
            .find(|t| t.id == track_id)
            .ok_or(EditError::TrackNotFound)?;
        match flag {
            TrackFlag::Lock => track.locked = value,
            TrackFlag::Mute => track.muted = value,
            TrackFlag::Solo => track.solo = value,
            TrackFlag::SyncLock => track.sync_lock = value,
        }
        Ok(())
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackFlag {
    Lock,
    Mute,
    Solo,
    SyncLock,
}

pub fn add_marker(
    project: &mut Project,
    sequence_id: SequenceId,
    frame: Frame,
    name: impl Into<String>,
) -> Result<MarkerId, EditError> {
    let mut id = MarkerId(0);
    let name = name.into();
    map_sequence(project, sequence_id, |sequence, alloc| {
        let marker_id = MarkerId(alloc());
        id = marker_id;
        sequence.markers.push(Marker {
            id: marker_id,
            frame,
            duration: 0,
            name,
            color: crate::model::LabelColor::Amber,
            comment: String::new(),
        });
        sequence.markers.sort_by_key(|m| m.frame.0);
        Ok(())
    })?;
    Ok(id)
}

pub fn set_in_point(
    project: &mut Project,
    sequence_id: SequenceId,
    frame: Option<Frame>,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        sequence.in_point = frame;
        Ok(())
    })
}

pub fn set_out_point(
    project: &mut Project,
    sequence_id: SequenceId,
    frame: Option<Frame>,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        sequence.out_point = frame;
        Ok(())
    })
}

pub fn import_media(project: &mut Project, mut asset: MediaAsset) -> MediaId {
    if asset.id.0 == 0 {
        asset.id = MediaId(project.alloc());
    } else {
        project.next_id = project.next_id.max(asset.id.0.saturating_add(1));
    }
    if asset.bin_id.0 == 0 {
        if let Some(bin) = project.bins.first() {
            asset.bin_id = bin.id;
        }
    }
    let id = asset.id;
    project.media.push(asset);
    id
}

/// Build a timeline clip that uses `source_in..source_out` of `media`.
pub fn clip_from_media(
    id: ClipId,
    media: &MediaAsset,
    sequence_timebase: Timebase,
    timeline_in: Frame,
    source_in: Frame,
    source_out: Frame,
    name: impl Into<String>,
) -> Result<Clip, EditError> {
    if source_out.0 <= source_in.0 {
        return Err(EditError::InvalidDuration);
    }
    let src_span = source_out.0 - source_in.0;
    let mut timeline_span = convert_frames(src_span, media.timebase, sequence_timebase);
    if timeline_span < 1 {
        timeline_span = 1;
    }
    Ok(Clip {
        id,
        media_id: Some(media.id),
        name: name.into(),
        timeline_in,
        timeline_out: Frame(timeline_in.0 + timeline_span),
        source_in,
        source_out,
        source_min: Frame::ZERO,
        source_max: media.duration,
        media_timebase: media.timebase,
        linked: Vec::new(),
        enabled: true,
        effects: Vec::new(),
        label: if media.has_video {
            crate::model::LabelColor::Blue
        } else {
            crate::model::LabelColor::Green
        },
        volume: crate::effects::AnimatedF32::constant(1.0),
    })
}

pub fn link_clips(a: &mut Clip, b: &mut Clip) {
    if !a.linked.contains(&b.id) {
        a.linked.push(b.id);
    }
    if !b.linked.contains(&a.id) {
        b.linked.push(a.id);
    }
}

fn source_delta(clip: &Clip, timeline_delta: i64, sequence_timebase: Timebase) -> i64 {
    // Prefer the clip's own source/timeline ratio so a 1x clip stays 1:1 even
    // after trims, and fall back to the timebase mapping when the spans match
    // a pure rate conversion.
    let timeline_span = clip.duration();
    let source_span = clip.source_duration();
    if timeline_span > 0 && source_span > 0 {
        let mapped = convert_frames(timeline_span, sequence_timebase, clip.media_timebase);
        if mapped == source_span {
            return convert_frames(timeline_delta, sequence_timebase, clip.media_timebase);
        }
        return mul_div_round(timeline_delta, source_span, timeline_span);
    }
    convert_frames(timeline_delta, sequence_timebase, clip.media_timebase)
}

/// Media frame of `clip` corresponding to `timeline_frame` on the sequence.
pub fn source_frame_at(clip: &Clip, timeline_frame: Frame, sequence_timebase: Timebase) -> Frame {
    let delta = timeline_frame.0 - clip.timeline_in.0;
    Frame(clip.source_in.0 + source_delta(clip, delta, sequence_timebase))
}

fn overlap_pair(duration: i64, alignment: TransitionAlign) -> (i64, i64) {
    match alignment {
        TransitionAlign::Center => {
            let left = duration / 2;
            (left, duration - left)
        }
        TransitionAlign::StartOnCut => (0, duration),
        TransitionAlign::EndOnCut => (duration, 0),
    }
}

fn place_overwrite(
    sequence: &mut Sequence,
    track_id: TrackId,
    clip: Clip,
    alloc: &mut dyn FnMut() -> u64,
) -> Result<(), EditError> {
    let timebase = sequence.timebase;
    let ti = sequence
        .track_index(track_id)
        .ok_or(EditError::TrackNotFound)?;
    if sequence.tracks[ti].locked {
        return Err(EditError::TrackLocked);
    }
    if sequence.tracks[ti].kind == TrackKind::Caption {
        return Err(EditError::WrongTrackKind);
    }
    let start = clip.timeline_in.0;
    let end = clip.timeline_out.0;
    if end <= start {
        return Err(EditError::InvalidDuration);
    }
    if start < 0 {
        return Err(EditError::OutOfRange);
    }
    let existing = std::mem::take(&mut sequence.tracks[ti].clips);
    let mut survivors = Vec::new();
    let mut removed = Vec::new();
    for mut current in existing {
        let cs = current.timeline_in.0;
        let ce = current.timeline_out.0;
        if ce <= start || cs >= end {
            survivors.push(current);
            continue;
        }
        let has_left = cs < start;
        let has_right = ce > end;
        if has_left && has_right {
            let mut right = current.clone();
            trim_tail_to(&mut current, start, timebase);
            trim_head_to(&mut right, end, timebase);
            right.id = ClipId(alloc());
            right.linked.clear();
            if current.duration() >= 1 {
                survivors.push(current);
            } else {
                removed.push(current.id);
            }
            if right.duration() >= 1 {
                survivors.push(right);
            }
        } else if has_left {
            trim_tail_to(&mut current, start, timebase);
            if current.duration() >= 1 {
                survivors.push(current);
            } else {
                removed.push(current.id);
            }
        } else if has_right {
            trim_head_to(&mut current, end, timebase);
            if current.duration() >= 1 {
                survivors.push(current);
            } else {
                removed.push(current.id);
            }
        } else {
            removed.push(current.id);
        }
    }
    survivors.push(clip);
    survivors.sort_by_key(|c| (c.timeline_in.0, c.id.0));
    sequence.tracks[ti].clips = survivors;
    if !removed.is_empty() {
        strip_ids(sequence, &removed);
    }
    Ok(())
}

fn trim_tail_to(clip: &mut Clip, new_out: i64, timebase: Timebase) {
    let delta = new_out - clip.timeline_out.0;
    let src = source_delta(clip, delta, timebase);
    clip.timeline_out = Frame(new_out);
    clip.source_out = Frame(clip.source_out.0 + src);
}

fn trim_head_to(clip: &mut Clip, new_in: i64, timebase: Timebase) {
    let delta = new_in - clip.timeline_in.0;
    let src = source_delta(clip, delta, timebase);
    clip.timeline_in = Frame(new_in);
    clip.source_in = Frame(clip.source_in.0 + src);
}

fn split_and_shift(
    sequence: &mut Sequence,
    at: i64,
    delta: i64,
    force_tracks: &[TrackId],
    alloc: &mut dyn FnMut() -> u64,
) -> Result<(), EditError> {
    let mut pairs = Vec::new();
    let track_count = sequence.tracks.len();
    for ti in 0..track_count {
        let forced = force_tracks.contains(&sequence.tracks[ti].id);
        let participate = !sequence.tracks[ti].locked && (sequence.tracks[ti].sync_lock || forced);
        if !participate {
            continue;
        }
        let spanning: Vec<ClipId> = sequence.tracks[ti]
            .clips
            .iter()
            .filter(|c| c.timeline_in.0 < at && c.timeline_out.0 > at)
            .map(|c| c.id)
            .collect();
        for id in spanning {
            let right = split_clip(sequence, id, Frame(at), alloc)?;
            pairs.push((id, right));
        }
        let cue_spans: Vec<CueId> = sequence.tracks[ti]
            .cues
            .iter()
            .filter(|c| c.timeline_in.0 < at && c.timeline_out.0 > at)
            .map(|c| c.id)
            .collect();
        for cue_id in cue_spans {
            split_cue(sequence, cue_id, at, alloc);
        }
        for clip in &mut sequence.tracks[ti].clips {
            if clip.timeline_in.0 >= at {
                clip.timeline_in.0 += delta;
                clip.timeline_out.0 += delta;
            }
        }
        for cue in &mut sequence.tracks[ti].cues {
            if cue.timeline_in.0 >= at {
                cue.timeline_in.0 += delta;
                cue.timeline_out.0 += delta;
            }
        }
        sequence.tracks[ti]
            .clips
            .sort_by_key(|c| (c.timeline_in.0, c.id.0));
    }
    for marker in &mut sequence.markers {
        if marker.frame.0 >= at {
            marker.frame.0 += delta;
        }
    }
    relink_splits(sequence, &pairs);
    Ok(())
}

fn split_clip(
    sequence: &mut Sequence,
    clip_id: ClipId,
    at: Frame,
    alloc: &mut dyn FnMut() -> u64,
) -> Result<ClipId, EditError> {
    let timebase = sequence.timebase;
    let (ti, ci) = sequence
        .locate_clip(clip_id)
        .ok_or(EditError::ClipNotFound)?;
    if sequence.tracks[ti].locked {
        return Err(EditError::TrackLocked);
    }
    let clip = &sequence.tracks[ti].clips[ci];
    if at.0 <= clip.timeline_in.0 || at.0 >= clip.timeline_out.0 {
        return Err(EditError::NotInsideClip);
    }
    let mut right = clip.clone();
    trim_tail_to(&mut sequence.tracks[ti].clips[ci], at.0, timebase);
    trim_head_to(&mut right, at.0, timebase);
    // trim_head_to / trim_tail_to use source_delta based on the pre-trim ratio.
    // The right clip was cloned before the left trim, so its ratio is the original.
    // But trim_tail_to already changed the left clip. Right was cloned before that,
    // then trim_head_to runs on the clone. Good.
    // However trim_tail_to on the left uses source_delta which reads the clip before
    // mutation inside the function via &mut... source_delta takes &Clip at start of
    // trim_tail_to before fields change. Good.
    let right_id = ClipId(alloc());
    right.id = right_id;
    sequence.tracks[ti].clips.push(right);
    sequence.tracks[ti]
        .clips
        .sort_by_key(|c| (c.timeline_in.0, c.id.0));
    for transition in &mut sequence.tracks[ti].transitions {
        if transition.left_clip == clip_id {
            transition.left_clip = right_id;
        }
    }
    Ok(right_id)
}

fn split_cue(sequence: &mut Sequence, cue_id: CueId, at: i64, alloc: &mut dyn FnMut() -> u64) {
    for track in &mut sequence.tracks {
        let Some(index) = track.cues.iter().position(|c| c.id == cue_id) else {
            continue;
        };
        let mut right = track.cues[index].clone();
        track.cues[index].timeline_out = Frame(at);
        right.id = CueId(alloc());
        right.timeline_in = Frame(at);
        track.cues.push(right);
        return;
    }
}

fn relink_splits(sequence: &mut Sequence, pairs: &[(ClipId, ClipId)]) {
    if pairs.is_empty() {
        return;
    }
    let lefts: std::collections::HashSet<ClipId> = pairs.iter().map(|(l, _)| *l).collect();
    let right_of: std::collections::HashMap<ClipId, ClipId> = pairs.iter().cloned().collect();
    let snapshot: Vec<(ClipId, Vec<ClipId>)> = sequence
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(|c| (c.id, c.linked.clone())))
        .collect();
    for (id, links) in snapshot {
        let rewritten: Vec<ClipId> = links
            .into_iter()
            .filter_map(|partner| {
                let is_right = right_of.values().any(|r| *r == id);
                if is_right {
                    right_of
                        .get(&partner)
                        .copied()
                        .or(Some(partner).filter(|p| !lefts.contains(p)))
                } else if lefts.contains(&id) {
                    if lefts.contains(&partner) || !right_of.contains_key(&partner) {
                        Some(partner)
                    } else {
                        Some(partner)
                    }
                } else if let Some(right) = right_of.get(&partner) {
                    // An unsplit clip that pointed at a split clip keeps the left id,
                    // which still exists. Drop a dangling right id if it was copied.
                    let _ = right;
                    if right_of.values().any(|r| *r == partner) {
                        None
                    } else {
                        Some(partner)
                    }
                } else {
                    Some(partner)
                }
            })
            .collect();
        let mut dedup = Vec::new();
        for link in rewritten {
            if link != id && !dedup.contains(&link) {
                dedup.push(link);
            }
        }
        if let Some((ti, ci)) = sequence.locate_clip(id) {
            sequence.tracks[ti].clips[ci].linked = dedup;
        }
    }
}

fn ripple_downstream(
    sequence: &mut Sequence,
    origin: TrackId,
    at: i64,
    delta: i64,
    skip: ClipId,
) -> Result<(), EditError> {
    let timebase = sequence.timebase;
    for track in &mut sequence.tracks {
        if track.locked {
            if track.id == origin {
                return Err(EditError::TrackLocked);
            }
            continue;
        }
        if track.id != origin && !track.sync_lock {
            continue;
        }
        for clip in &mut track.clips {
            if clip.id == skip {
                continue;
            }
            if clip.timeline_in.0 >= at {
                clip.timeline_in.0 += delta;
                clip.timeline_out.0 += delta;
                if clip.timeline_in.0 < 0 {
                    return Err(EditError::OutOfRange);
                }
            } else if clip.timeline_out.0 > at {
                let src = source_delta(clip, delta, timebase);
                let new_out = clip.timeline_out.0 + delta;
                let new_source = clip.source_out.0 + src;
                if new_out <= clip.timeline_in.0 {
                    return Err(EditError::InvalidDuration);
                }
                if new_source > clip.source_max.0 || new_source <= clip.source_in.0 {
                    return Err(EditError::InsufficientHandle {
                        have: clip.tail_handle(),
                        need: src.abs(),
                    });
                }
                clip.timeline_out = Frame(new_out);
                clip.source_out = Frame(new_source);
            }
        }
        for cue in &mut track.cues {
            if cue.timeline_in.0 >= at {
                cue.timeline_in.0 += delta;
                cue.timeline_out.0 += delta;
            } else if cue.timeline_out.0 > at {
                cue.timeline_out.0 += delta;
                if cue.timeline_out.0 <= cue.timeline_in.0 {
                    return Err(EditError::InvalidDuration);
                }
            }
        }
    }
    for marker in &mut sequence.markers {
        if marker.frame.0 >= at {
            marker.frame.0 += delta;
            if marker.frame.0 < 0 {
                return Err(EditError::OutOfRange);
            }
        }
    }
    Ok(())
}

fn ensure_unlocked(sequence: &Sequence, clip_ids: &[ClipId]) -> Result<(), EditError> {
    for id in clip_ids {
        if let Some((ti, _)) = sequence.locate_clip(*id) {
            if sequence.tracks[ti].locked {
                return Err(EditError::TrackLocked);
            }
        }
    }
    Ok(())
}

fn remove_clips(sequence: &mut Sequence, clip_ids: &[ClipId]) {
    let set: std::collections::HashSet<ClipId> = clip_ids.iter().copied().collect();
    for track in &mut sequence.tracks {
        track.clips.retain(|c| !set.contains(&c.id));
        track
            .transitions
            .retain(|t| !set.contains(&t.left_clip) && !set.contains(&t.right_clip));
        for clip in &mut track.clips {
            clip.linked.retain(|id| !set.contains(id));
        }
    }
}

fn take_clip(sequence: &mut Sequence, id: ClipId) -> Result<(TrackId, Clip), EditError> {
    let (ti, ci) = sequence.locate_clip(id).ok_or(EditError::ClipNotFound)?;
    let track_id = sequence.tracks[ti].id;
    let clip = sequence.tracks[ti].clips.remove(ci);
    for track in &mut sequence.tracks {
        track
            .transitions
            .retain(|t| t.left_clip != id && t.right_clip != id);
    }
    Ok((track_id, clip))
}

fn strip_ids(sequence: &mut Sequence, removed: &[ClipId]) {
    let set: std::collections::HashSet<ClipId> = removed.iter().copied().collect();
    for track in &mut sequence.tracks {
        for clip in &mut track.clips {
            clip.linked.retain(|id| !set.contains(id));
        }
        track
            .transitions
            .retain(|t| !set.contains(&t.left_clip) && !set.contains(&t.right_clip));
    }
}

fn cleanup_transitions(sequence: &mut Sequence) {
    for track in &mut sequence.tracks {
        let clips = track.clips.clone();
        track.transitions.retain(|transition| {
            let Some(left) = clips.iter().find(|c| c.id == transition.left_clip) else {
                return false;
            };
            let Some(right) = clips.iter().find(|c| c.id == transition.right_clip) else {
                return false;
            };
            left.timeline_out == right.timeline_in
        });
    }
}

fn linked_group(sequence: &Sequence, clip_id: ClipId) -> Vec<ClipId> {
    let mut group = vec![clip_id];
    if let Some(clip) = sequence.clip(clip_id) {
        for id in &clip.linked {
            if sequence.clip(*id).is_some() && !group.contains(id) {
                group.push(*id);
            }
        }
    }
    group
}

/// Expand a selection through linked clips. Unknown ids are dropped.
pub fn expand_linked(sequence: &Sequence, clip_ids: &[ClipId]) -> Vec<ClipId> {
    let mut out = Vec::new();
    for id in clip_ids {
        for linked in linked_group(sequence, *id) {
            if !out.contains(&linked) {
                out.push(linked);
            }
        }
    }
    out
}

pub fn active_sequence_id(project: &Project) -> Result<SequenceId, EditError> {
    active_id(project)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        Clip, ClipId, Direction, LabelColor, Marker, MarkerId, SequenceId, TrackId, TrackKind,
        TransitionAlign, TransitionKind,
    };
    use crate::time::{Frame, Timebase};

    fn project_with(sequence: Sequence) -> Project {
        let mut project = Project::new("test");
        project.active_sequence = Some(sequence.id);
        project.next_id = 50_000;
        project.sequences.push(sequence);
        project
    }

    #[test]
    fn source_frame_follows_the_playhead() {
        let clip = Clip::basic(1, 10, 40);
        assert_eq!(source_frame_at(&clip, Frame(10), Timebase::fps_24()).0, 0);
        assert_eq!(source_frame_at(&clip, Frame(25), Timebase::fps_24()).0, 15);
        let handled = clip.with_handles(5, 5);
        assert_eq!(
            source_frame_at(&handled, Frame(10), Timebase::fps_24()).0,
            5
        );
        assert_eq!(
            source_frame_at(&handled, Frame(25), Timebase::fps_24()).0,
            20
        );
    }

    fn video_sequence() -> (Sequence, TrackId) {
        let mut sequence = Sequence::new(SequenceId(1), "T", 1920, 1080, Timebase::fps_24());
        let track = sequence.add_track(TrackId(2), TrackKind::Video, "V1");
        sequence.tracks[0].sync_lock = true;
        (sequence, track)
    }

    fn ranges(sequence: &Sequence, track: TrackId) -> Vec<(u64, i64, i64)> {
        sequence
            .track(track)
            .unwrap()
            .clips
            .iter()
            .map(|c| (c.id.0, c.timeline_in.0, c.timeline_out.0))
            .collect()
    }

    #[test]
    fn overwrite_trims_neighbours_and_splits_spanning() {
        let (mut sequence, track) = video_sequence();
        sequence.tracks[0].clips = vec![Clip::basic(1, 0, 100), Clip::basic(2, 100, 200)];
        let mut project = project_with(sequence);
        overwrite_clips(
            &mut project,
            SequenceId(1),
            vec![(track, Clip::basic(9, 50, 150))],
        )
        .unwrap();
        let seq = project.active().unwrap();
        assert_eq!(
            ranges(seq, track),
            vec![(1, 0, 50), (9, 50, 150), (2, 150, 200)]
        );
        let left = seq.clip(ClipId(1)).unwrap();
        assert_eq!(left.source_out.0, 50);
        let right = seq.clip(ClipId(2)).unwrap();
        assert_eq!(right.source_in.0, 50);
        assert_eq!(right.source_out.0, 100);

        let (mut sequence, track) = video_sequence();
        sequence.tracks[0].clips = vec![Clip::basic(1, 0, 200)];
        let mut project = project_with(sequence);
        overwrite_clips(
            &mut project,
            SequenceId(1),
            vec![(track, Clip::basic(9, 80, 120))],
        )
        .unwrap();
        let seq = project.active().unwrap();
        let clips = &seq.track(track).unwrap().clips;
        assert_eq!(clips.len(), 3);
        assert_eq!(clips[0].timeline_in.0, 0);
        assert_eq!(clips[0].timeline_out.0, 80);
        assert_eq!(clips[0].source_out.0, 80);
        assert_eq!(clips[1].id, ClipId(9));
        assert_eq!(clips[2].timeline_in.0, 120);
        assert_eq!(clips[2].timeline_out.0, 200);
        assert_eq!(clips[2].source_in.0, 120);
        assert_eq!(clips[2].source_out.0, 200);
        assert_ne!(clips[2].id, ClipId(1));
    }

    #[test]
    fn insert_splits_and_shifts_sync_locked_audio() {
        let mut sequence = Sequence::new(SequenceId(1), "T", 1920, 1080, Timebase::fps_24());
        let video = sequence.add_track(TrackId(2), TrackKind::Video, "V1");
        let audio = sequence.add_track(TrackId(3), TrackKind::Audio, "A1");
        sequence.tracks[0].clips = vec![Clip::basic(1, 0, 100), Clip::basic(2, 100, 200)];
        sequence.tracks[1].clips = vec![Clip::basic(3, 0, 100)];
        sequence.markers.push(Marker {
            id: MarkerId(7),
            frame: Frame(50),
            duration: 0,
            name: "mark".into(),
            color: LabelColor::Amber,
            comment: String::new(),
        });
        let mut project = project_with(sequence);
        insert_clips(
            &mut project,
            SequenceId(1),
            vec![(video, Clip::basic(9, 40, 50))],
        )
        .unwrap();
        let seq = project.active().unwrap();
        let video_clips = &seq.track(video).unwrap().clips;
        assert_eq!(video_clips[0].timeline_in.0, 0);
        assert_eq!(video_clips[0].timeline_out.0, 40);
        assert_eq!(video_clips[0].source_out.0, 40);
        assert_eq!(video_clips[1].id, ClipId(9));
        assert_eq!(video_clips[1].timeline_in.0, 40);
        assert_eq!(video_clips[1].timeline_out.0, 50);
        assert_eq!(video_clips[2].timeline_in.0, 50);
        assert_eq!(video_clips[2].timeline_out.0, 110);
        assert_eq!(video_clips[2].source_in.0, 40);
        assert_eq!(video_clips[3].timeline_in.0, 110);
        assert_eq!(video_clips[3].timeline_out.0, 210);
        let audio_clips = &seq.track(audio).unwrap().clips;
        assert_eq!(audio_clips.len(), 2);
        assert_eq!(audio_clips[0].timeline_out.0, 40);
        assert_eq!(audio_clips[1].timeline_in.0, 50);
        assert_eq!(audio_clips[1].timeline_out.0, 110);
        assert_eq!(seq.markers[0].frame, Frame(60));
    }

    #[test]
    fn razor_preserves_source_and_linked_pair() {
        let mut sequence = Sequence::new(SequenceId(1), "T", 1920, 1080, Timebase::fps_24());
        let video = sequence.add_track(TrackId(2), TrackKind::Video, "V1");
        let audio = sequence.add_track(TrackId(3), TrackKind::Audio, "A1");
        let mut v = Clip::basic(1, 0, 100);
        let mut a = Clip::basic(2, 0, 100);
        link_clips(&mut v, &mut a);
        sequence.tracks[0].clips = vec![v];
        sequence.tracks[1].clips = vec![a];
        let mut project = project_with(sequence);
        razor_clip(&mut project, SequenceId(1), ClipId(1), Frame(40)).unwrap();
        let seq = project.active().unwrap();
        let vclips = &seq.track(video).unwrap().clips;
        let aclips = &seq.track(audio).unwrap().clips;
        assert_eq!(vclips.len(), 2);
        assert_eq!(aclips.len(), 2);
        assert_eq!((vclips[0].source_in.0, vclips[0].source_out.0), (0, 40));
        assert_eq!((vclips[1].source_in.0, vclips[1].source_out.0), (40, 100));
        assert_eq!(vclips[0].timeline_out, vclips[1].timeline_in);
        assert_eq!(vclips[0].linked, vec![aclips[0].id]);
        assert_eq!(vclips[1].linked, vec![aclips[1].id]);
        assert_eq!(aclips[0].linked, vec![vclips[0].id]);
        assert_eq!(aclips[1].linked, vec![vclips[1].id]);
        let left_id = vclips[0].id;
        assert!(razor_clip(&mut project, SequenceId(1), left_id, Frame(0)).is_err());
    }

    #[test]
    fn lift_leaves_gap_and_ripple_closes_it() {
        let (mut sequence, track) = video_sequence();
        sequence.tracks[0].clips = vec![
            Clip::basic(1, 0, 50),
            Clip::basic(2, 50, 100),
            Clip::basic(3, 100, 130),
        ];
        let mut lifted = project_with(sequence.clone());
        lift_delete(&mut lifted, SequenceId(1), &[ClipId(2)]).unwrap();
        assert_eq!(
            ranges(lifted.active().unwrap(), track),
            vec![(1, 0, 50), (3, 100, 130)]
        );
        let mut rippled = project_with(sequence);
        ripple_delete(&mut rippled, SequenceId(1), &[ClipId(2)]).unwrap();
        assert_eq!(
            ranges(rippled.active().unwrap(), track),
            vec![(1, 0, 50), (3, 50, 80)]
        );
    }

    #[test]
    fn move_overwrites_destination() {
        let (mut sequence, track) = video_sequence();
        sequence.tracks[0].clips = vec![Clip::basic(1, 0, 50), Clip::basic(2, 100, 180)];
        let mut project = project_with(sequence);
        move_clips(&mut project, SequenceId(1), &[ClipId(1)], 100, None).unwrap();
        assert_eq!(
            ranges(project.active().unwrap(), track),
            vec![(1, 100, 150), (2, 150, 180)]
        );
    }

    #[test]
    fn snap_picks_nearest_edge_within_threshold() {
        let targets = [
            SnapPoint {
                frame: Frame(100),
                kind: SnapKind::ClipEdge,
            },
            SnapPoint {
                frame: Frame(200),
                kind: SnapKind::ClipEdge,
            },
        ];
        let hit = snap_to_targets(Frame(105), &targets, 8).unwrap();
        assert_eq!(hit.frame, Frame(100));
        assert!(snap_to_targets(Frame(105), &targets, 4).is_none());
        assert_eq!(
            snap_to_targets(Frame(190), &targets, 12).unwrap().frame,
            Frame(200)
        );
        let snapped = snap_span(Frame(96), 20, &targets, 5);
        assert_eq!(snapped, Frame(100));
    }

    #[test]
    fn ripple_trim_head_and_tail() {
        let (mut sequence, track) = video_sequence();
        sequence.tracks[0].clips = vec![
            Clip::basic(1, 0, 100).with_handles(0, 40),
            Clip::basic(2, 100, 180).with_handles(20, 20),
        ];
        let mut project = project_with(sequence);
        ripple_trim(&mut project, SequenceId(1), ClipId(1), TrimEdge::Tail, 10).unwrap();
        let seq = project.active().unwrap();
        let a = seq.clip(ClipId(1)).unwrap();
        let b = seq.clip(ClipId(2)).unwrap();
        assert_eq!((a.timeline_in.0, a.timeline_out.0), (0, 110));
        assert_eq!(a.source_out.0, 110);
        assert_eq!((b.timeline_in.0, b.timeline_out.0), (110, 190));

        ripple_trim(&mut project, SequenceId(1), ClipId(2), TrimEdge::Head, 10).unwrap();
        let seq = project.active().unwrap();
        let b = seq.clip(ClipId(2)).unwrap();
        assert_eq!((b.timeline_in.0, b.timeline_out.0), (110, 200));
        assert_eq!(b.source_in.0, 10);
        assert_eq!(ranges(seq, track).last().unwrap().2, 200);

        let err = ripple_trim(&mut project, SequenceId(1), ClipId(2), TrimEdge::Head, 100);
        assert!(matches!(err, Err(EditError::InsufficientHandle { .. })));
    }

    #[test]
    fn roll_keeps_combined_span() {
        let (mut sequence, track) = video_sequence();
        sequence.tracks[0].clips = vec![
            Clip::basic(1, 0, 100).with_handles(0, 80),
            Clip::basic(2, 100, 200).with_handles(40, 10),
        ];
        let mut project = project_with(sequence);
        roll_cut(&mut project, SequenceId(1), ClipId(1), 10).unwrap();
        let seq = project.active().unwrap();
        let a = seq.clip(ClipId(1)).unwrap();
        let b = seq.clip(ClipId(2)).unwrap();
        assert_eq!((a.timeline_in.0, a.timeline_out.0), (0, 110));
        assert_eq!(a.source_out.0, 110);
        assert_eq!((b.timeline_in.0, b.timeline_out.0), (110, 200));
        assert_eq!(b.source_in.0, 50);
        assert_eq!(ranges(seq, track)[0].1, 0);
        assert_eq!(ranges(seq, track)[1].2, 200);
        assert!(roll_cut(&mut project, SequenceId(1), ClipId(1), 10_000).is_err());
    }

    #[test]
    fn slip_changes_source_only() {
        let (mut sequence, _track) = video_sequence();
        sequence.tracks[0].clips = vec![Clip::basic(1, 0, 50).with_handles(10, 40)];
        let mut project = project_with(sequence);
        slip(&mut project, SequenceId(1), ClipId(1), 5).unwrap();
        let clip = project.active().unwrap().clip(ClipId(1)).unwrap();
        assert_eq!((clip.timeline_in.0, clip.timeline_out.0), (0, 50));
        assert_eq!((clip.source_in.0, clip.source_out.0), (15, 65));
        slip(&mut project, SequenceId(1), ClipId(1), -15).unwrap();
        let clip = project.active().unwrap().clip(ClipId(1)).unwrap();
        assert_eq!((clip.source_in.0, clip.source_out.0), (0, 50));
        assert!(slip(&mut project, SequenceId(1), ClipId(1), -1).is_err());
    }

    #[test]
    fn slide_keeps_outer_span() {
        let (mut sequence, track) = video_sequence();
        sequence.tracks[0].clips = vec![
            Clip::basic(1, 0, 100).with_handles(0, 50),
            Clip::basic(2, 100, 150).with_handles(0, 0),
            Clip::basic(3, 150, 250).with_handles(20, 10),
        ];
        let mut project = project_with(sequence);
        slide(&mut project, SequenceId(1), ClipId(2), 10).unwrap();
        let seq = project.active().unwrap();
        let a = seq.clip(ClipId(1)).unwrap();
        let b = seq.clip(ClipId(2)).unwrap();
        let c = seq.clip(ClipId(3)).unwrap();
        assert_eq!((a.timeline_in.0, a.timeline_out.0), (0, 110));
        assert_eq!(a.source_out.0, 110);
        assert_eq!((b.timeline_in.0, b.timeline_out.0), (110, 160));
        assert_eq!((b.source_in.0, b.source_out.0), (0, 50));
        assert_eq!((c.timeline_in.0, c.timeline_out.0), (160, 250));
        assert_eq!(c.source_in.0, 30);
        assert_eq!(ranges(seq, track)[0].1, 0);
        assert_eq!(ranges(seq, track)[2].2, 250);
        assert!(slide(&mut project, SequenceId(1), ClipId(1), 5).is_err());
    }

    #[test]
    fn transition_centres_on_the_cut_and_needs_handles() {
        let (mut sequence, _track) = video_sequence();
        sequence.tracks[0].clips = vec![
            Clip::basic(1, 0, 100).with_handles(0, 20),
            Clip::basic(2, 100, 200).with_handles(20, 0),
        ];
        let mut project = project_with(sequence);
        let id = add_transition(
            &mut project,
            SequenceId(1),
            ClipId(1),
            TransitionKind::CrossDissolve,
            16,
            TransitionAlign::Center,
        )
        .unwrap();
        let transition = project.active().unwrap().tracks[0]
            .transitions
            .iter()
            .find(|t| t.id == id)
            .unwrap();
        assert_eq!(transition.range(Frame(100)), (Frame(92), Frame(108)));
        assert_eq!(transition.progress(Frame(100), Frame(100)), Some(0.5));

        let (mut sequence, _track) = video_sequence();
        sequence.tracks[0].clips = vec![
            Clip::basic(1, 0, 100).with_handles(0, 2),
            Clip::basic(2, 100, 200).with_handles(2, 0),
        ];
        let mut project = project_with(sequence);
        let err = add_transition(
            &mut project,
            SequenceId(1),
            ClipId(1),
            TransitionKind::Wipe { angle_deg: 90.0 },
            16,
            TransitionAlign::Center,
        );
        assert!(matches!(err, Err(EditError::InsufficientHandle { .. })));
    }

    #[test]
    fn locked_track_rejects_edits() {
        let (mut sequence, track) = video_sequence();
        sequence.tracks[0].locked = true;
        sequence.tracks[0].clips = vec![Clip::basic(1, 0, 10)];
        let mut project = project_with(sequence);
        assert_eq!(
            lift_delete(&mut project, SequenceId(1), &[ClipId(1)]),
            Err(EditError::TrackLocked)
        );
        assert_eq!(
            overwrite_clips(
                &mut project,
                SequenceId(1),
                vec![(track, Clip::basic(2, 0, 5))]
            ),
            Err(EditError::TrackLocked)
        );
    }

    #[test]
    fn push_slide_kind_is_distinct() {
        let kind = TransitionKind::PushSlide {
            direction: Direction::Left,
        };
        assert_eq!(kind.label(), "Push");
    }

    #[test]
    fn imported_media_path_survives_project_json() {
        use crate::model::{Bin, BinId, MediaAsset, MediaId};

        let mut project = Project::new("import");
        let bin = BinId(project.alloc());
        project.bins.push(Bin {
            id: bin,
            name: "Master".into(),
            parent: None,
        });
        let asset = MediaAsset {
            id: MediaId(0),
            bin_id: BinId(0),
            name: "interview.mp4".into(),
            path: "/home/editor/footage/interview.mp4".into(),
            duration: Frame(240),
            timebase: Timebase::fps_24(),
            width: Some(1920),
            height: Some(1080),
            video_codec: Some("h264".into()),
            audio_codec: Some("aac".into()),
            audio_channels: Some(2),
            sample_rate: Some(48_000),
            has_video: true,
            has_audio: true,
            offline: false,
        };
        let id = import_media(&mut project, asset);
        let json = project.to_json_pretty().unwrap();
        let loaded = Project::from_json(&json).unwrap();
        let media = loaded.media(id).unwrap();
        assert_eq!(media.path, "/home/editor/footage/interview.mp4");
        assert_eq!(media.name, "interview.mp4");
        assert_eq!(media.bin_id, bin);
        assert!(media.has_video && media.has_audio);
        assert!(!media.offline);
        assert_eq!(media.duration, Frame(240));
    }
}
