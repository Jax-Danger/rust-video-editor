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
use crate::effects::{
    blur_mut, chroma_key_mut, color_grade_mut, crop_mut, lut_mut, shape_mask_mut, sharpen_mut,
    stabilize_mut, transform_mut, vignette_mut, GradeParam, TransformParam,
};
use crate::model::{
    Bin, BinId, CaptionCue, Clip, ClipId, ClipSpeed, CueId, LabelColor, Marker, MarkerId,
    MediaAsset, MediaId, Project, Sequence, SequenceId, Title, Track, TrackId, TrackKind,
    Transition,
    TransitionAlign, TransitionId, TransitionKind,
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
    #[error("clip is not a title")]
    NotATitle,
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
    #[error("select at least two video angles")]
    NotEnoughAngles,
    #[error("clip is not a multicam clip")]
    NotMulticam,
    #[error("angle is out of range")]
    AngleOutOfRange,
    #[error("multicam group not found")]
    GroupNotFound,
    #[error("nothing selected")]
    NothingSelected,
    #[error("clip is already nested")]
    AlreadyNested,
    #[error("clip cannot be nested")]
    NotNestable,
    #[error("nested sequence would create a cycle")]
    NestedCycle,
    #[error("bin not found")]
    BinNotFound,
    #[error("marker not found")]
    MarkerNotFound,
    #[error("cannot delete the only bin")]
    LastBin,
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
            track.reindex();
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

/// Ripple-trim the previous edit on targeted tracks to `at`. Inside a clip this
/// removes media before the playhead and parks the cut on `at`. In a gap it
/// extends the previous clip's tail.
pub fn ripple_trim_prev_to_playhead(
    project: &mut Project,
    sequence_id: SequenceId,
    at: Frame,
    track_ids: &[TrackId],
) -> Result<(), EditError> {
    if track_ids.is_empty() {
        return Err(EditError::TrackNotFound);
    }
    map_sequence(project, sequence_id, |sequence, alloc| {
        let allowed: std::collections::HashSet<TrackId> = track_ids.iter().copied().collect();
        let mut changed = false;
        let containing = clips_containing_on_tracks(sequence, &allowed, at);
        if !containing.is_empty() {
            let targets = expand_split_targets(sequence, &containing, at);
            let mut pairs = Vec::new();
            for id in targets {
                if sequence.locate_clip(id).is_some() {
                    let right = split_clip(sequence, id, at, alloc)?;
                    pairs.push((id, right));
                }
            }
            relink_splits(sequence, &pairs);
            let lefts: Vec<ClipId> = pairs.iter().map(|(left, _)| *left).collect();
            ensure_unlocked(sequence, &lefts)?;
            remove_clips(sequence, &lefts);
            cleanup_transitions(sequence);
            changed = true;
        }
        for track_id in track_ids {
            let ti = sequence
                .tracks
                .iter()
                .position(|track| track.id == *track_id)
                .ok_or(EditError::TrackNotFound)?;
            if sequence.tracks[ti].locked {
                continue;
            }
            let track = &sequence.tracks[ti];
            if track.clips.iter().any(|clip| clip.contains_frame(at)) {
                continue;
            }
            if let Some(ci) = track.clips.iter().position(|clip| clip.timeline_in == at) {
                if ci > 0 {
                    let prev = &track.clips[ci - 1];
                    let delta = at.0 - prev.timeline_out.0;
                    if delta != 0 {
                        ripple_trim_in_sequence(sequence, prev.id, TrimEdge::Tail, delta)?;
                        changed = true;
                    }
                }
                continue;
            }
            if let Some(prev) = track.clips.iter().rfind(|clip| clip.timeline_out.0 < at.0) {
                let delta = at.0 - prev.timeline_out.0;
                if delta > 0 {
                    ripple_trim_in_sequence(sequence, prev.id, TrimEdge::Tail, delta)?;
                    changed = true;
                }
            }
        }
        if !changed {
            return Err(EditError::NotInsideClip);
        }
        Ok(())
    })
}

/// Ripple-trim the next edit on targeted tracks to `at`. Inside a clip this
/// removes media after the playhead. In a gap it pulls the next clip's head to
/// `at`.
pub fn ripple_trim_next_to_playhead(
    project: &mut Project,
    sequence_id: SequenceId,
    at: Frame,
    track_ids: &[TrackId],
) -> Result<(), EditError> {
    if track_ids.is_empty() {
        return Err(EditError::TrackNotFound);
    }
    map_sequence(project, sequence_id, |sequence, alloc| {
        let allowed: std::collections::HashSet<TrackId> = track_ids.iter().copied().collect();
        let mut changed = false;
        let containing = clips_containing_on_tracks(sequence, &allowed, at);
        if !containing.is_empty() {
            let targets = expand_split_targets(sequence, &containing, at);
            let mut pairs = Vec::new();
            for id in targets {
                if sequence.locate_clip(id).is_some() {
                    let right = split_clip(sequence, id, at, alloc)?;
                    pairs.push((id, right));
                }
            }
            relink_splits(sequence, &pairs);
            let rights: Vec<ClipId> = pairs.iter().map(|(_, right)| *right).collect();
            ensure_unlocked(sequence, &rights)?;
            remove_clips(sequence, &rights);
            cleanup_transitions(sequence);
            changed = true;
        }
        for track_id in track_ids {
            let ti = sequence
                .tracks
                .iter()
                .position(|track| track.id == *track_id)
                .ok_or(EditError::TrackNotFound)?;
            if sequence.tracks[ti].locked {
                continue;
            }
            let track = &sequence.tracks[ti];
            if track.clips.iter().any(|clip| clip.contains_frame(at)) {
                continue;
            }
            if let Some(next) = track.clips.iter().find(|clip| clip.timeline_in.0 > at.0) {
                let delta = at.0 - next.timeline_in.0;
                if delta != 0 {
                    trim_in_sequence(sequence, next.id, TrimEdge::Head, delta)?;
                    changed = true;
                }
            }
        }
        if !changed {
            return Err(EditError::NotInsideClip);
        }
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
        ripple_downstream_except(sequence, origin, old_out, delta, clip_id, &[])?;
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

/// Write all three RGB offsets for a lift / gamma / gain wheel at once.
pub fn set_wheel_offsets_at(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    wheel: crate::effects::WheelKind,
    clip_relative_frame: i64,
    red: f32,
    green: f32,
    blue: f32,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        let grade = color_grade_mut(&mut sequence.tracks[ti].clips[ci].effects);
        let wheel = grade.wheel_mut(wheel);
        wheel.red.write_at(clip_relative_frame, red);
        wheel.green.write_at(clip_relative_frame, green);
        wheel.blue.write_at(clip_relative_frame, blue);
        Ok(())
    })
}

/// Move an interior luma-curve control point. Endpoints stay fixed.
pub fn set_luma_curve_point(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    index: usize,
    output_y: f32,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        let grade = color_grade_mut(&mut sequence.tracks[ti].clips[ci].effects);
        grade.luma_curve.set_point_y(index, output_y);
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

pub fn set_filter_at(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    param: crate::effects::FilterParam,
    clip_relative_frame: i64,
    value: f32,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        let effects = &mut sequence.tracks[ti].clips[ci].effects;
        match param {
            crate::effects::FilterParam::BlurRadius => {
                blur_mut(effects)
                    .radius
                    .write_at(clip_relative_frame, value.max(0.0));
            }
            crate::effects::FilterParam::VignetteAmount => {
                vignette_mut(effects)
                    .amount
                    .write_at(clip_relative_frame, value.clamp(0.0, 1.0));
            }
            crate::effects::FilterParam::VignetteSoftness => {
                vignette_mut(effects)
                    .softness
                    .write_at(clip_relative_frame, value.clamp(0.05, 1.0));
            }
            crate::effects::FilterParam::CropLeft
            | crate::effects::FilterParam::CropRight
            | crate::effects::FilterParam::CropTop
            | crate::effects::FilterParam::CropBottom => {
                let crop = crop_mut(effects);
                let anim = match param {
                    crate::effects::FilterParam::CropLeft => &mut crop.left,
                    crate::effects::FilterParam::CropRight => &mut crop.right,
                    crate::effects::FilterParam::CropTop => &mut crop.top,
                    crate::effects::FilterParam::CropBottom => &mut crop.bottom,
                    _ => unreachable!(),
                };
                anim.write_at(clip_relative_frame, value.clamp(0.0, 0.45));
            }
            crate::effects::FilterParam::SharpenAmount => {
                sharpen_mut(effects)
                    .amount
                    .write_at(clip_relative_frame, value.clamp(0.0, 2.0));
            }
            crate::effects::FilterParam::ChromaKeyRed => {
                chroma_key_mut(effects)
                    .key_red
                    .write_at(clip_relative_frame, value.clamp(0.0, 1.0));
            }
            crate::effects::FilterParam::ChromaKeyGreen => {
                chroma_key_mut(effects)
                    .key_green
                    .write_at(clip_relative_frame, value.clamp(0.0, 1.0));
            }
            crate::effects::FilterParam::ChromaKeyBlue => {
                chroma_key_mut(effects)
                    .key_blue
                    .write_at(clip_relative_frame, value.clamp(0.0, 1.0));
            }
            crate::effects::FilterParam::ChromaKeyTolerance => {
                chroma_key_mut(effects)
                    .tolerance
                    .write_at(clip_relative_frame, value.clamp(0.0, 1.0));
            }
            crate::effects::FilterParam::ChromaKeySoftness => {
                chroma_key_mut(effects)
                    .softness
                    .write_at(clip_relative_frame, value.clamp(0.0, 1.0));
            }
            crate::effects::FilterParam::ChromaKeySpillSuppression => {
                chroma_key_mut(effects)
                    .spill_suppression
                    .write_at(clip_relative_frame, value.clamp(0.0, 1.0));
            }
            crate::effects::FilterParam::StabilizeStrength => {
                stabilize_mut(effects)
                    .strength
                    .write_at(clip_relative_frame, value.clamp(0.0, 1.0));
            }
            crate::effects::FilterParam::StabilizeSmoothing => {
                stabilize_mut(effects)
                    .smoothing
                    .write_at(clip_relative_frame, value.clamp(0.05, 1.0));
            }
            crate::effects::FilterParam::ShapeMaskCenterX => {
                shape_mask_mut(effects)
                    .center_x
                    .write_at(clip_relative_frame, value.clamp(0.0, 1.0));
            }
            crate::effects::FilterParam::ShapeMaskCenterY => {
                shape_mask_mut(effects)
                    .center_y
                    .write_at(clip_relative_frame, value.clamp(0.0, 1.0));
            }
            crate::effects::FilterParam::ShapeMaskWidth => {
                shape_mask_mut(effects)
                    .width
                    .write_at(clip_relative_frame, value.clamp(0.01, 1.0));
            }
            crate::effects::FilterParam::ShapeMaskHeight => {
                shape_mask_mut(effects)
                    .height
                    .write_at(clip_relative_frame, value.clamp(0.01, 1.0));
            }
            crate::effects::FilterParam::ShapeMaskFeather => {
                shape_mask_mut(effects)
                    .feather
                    .write_at(clip_relative_frame, value.clamp(0.0, 0.5));
            }
            crate::effects::FilterParam::LutMix => {
                lut_mut(effects)
                    .mix
                    .write_at(clip_relative_frame, value.clamp(0.0, 1.0));
            }
        }
        Ok(())
    })
}

pub fn set_lut_look(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    path: String,
    title: Option<String>,
    embedded: Option<crate::effects::EmbeddedLut3D>,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        let filter = lut_mut(&mut sequence.tracks[ti].clips[ci].effects);
        filter.path = path;
        filter.title = title;
        filter.embedded = embedded;
        Ok(())
    })
}

pub fn clear_lut(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        sequence.tracks[ti].clips[ci]
            .effects
            .retain(|effect| !matches!(effect, crate::effects::Effect::Lut(_)));
        Ok(())
    })
}

pub fn set_shape_mask_shape(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    shape: crate::effects::ShapeMaskKind,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        shape_mask_mut(&mut sequence.tracks[ti].clips[ci].effects).shape = shape;
        Ok(())
    })
}

pub fn set_shape_mask_invert(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    invert: bool,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        shape_mask_mut(&mut sequence.tracks[ti].clips[ci].effects).invert = invert;
        Ok(())
    })
}

pub fn set_track_matte(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    matte: Option<crate::model::TrackMatteBinding>,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        sequence.tracks[ti].clips[ci].track_matte = matte;
        Ok(())
    })
}

pub fn toggle_filter_key(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    param: crate::effects::FilterParam,
    clip_relative_frame: i64,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        let effects = &mut sequence.tracks[ti].clips[ci].effects;
        let anim = match param {
            crate::effects::FilterParam::BlurRadius => &mut blur_mut(effects).radius,
            crate::effects::FilterParam::VignetteAmount => &mut vignette_mut(effects).amount,
            crate::effects::FilterParam::VignetteSoftness => &mut vignette_mut(effects).softness,
            crate::effects::FilterParam::CropLeft => &mut crop_mut(effects).left,
            crate::effects::FilterParam::CropRight => &mut crop_mut(effects).right,
            crate::effects::FilterParam::CropTop => &mut crop_mut(effects).top,
            crate::effects::FilterParam::CropBottom => &mut crop_mut(effects).bottom,
            crate::effects::FilterParam::SharpenAmount => &mut sharpen_mut(effects).amount,
            crate::effects::FilterParam::ChromaKeyRed => &mut chroma_key_mut(effects).key_red,
            crate::effects::FilterParam::ChromaKeyGreen => &mut chroma_key_mut(effects).key_green,
            crate::effects::FilterParam::ChromaKeyBlue => &mut chroma_key_mut(effects).key_blue,
            crate::effects::FilterParam::ChromaKeyTolerance => &mut chroma_key_mut(effects).tolerance,
            crate::effects::FilterParam::ChromaKeySoftness => &mut chroma_key_mut(effects).softness,
            crate::effects::FilterParam::ChromaKeySpillSuppression => {
                &mut chroma_key_mut(effects).spill_suppression
            }
            crate::effects::FilterParam::StabilizeStrength => &mut stabilize_mut(effects).strength,
            crate::effects::FilterParam::StabilizeSmoothing => {
                &mut stabilize_mut(effects).smoothing
            }
            crate::effects::FilterParam::ShapeMaskCenterX => &mut shape_mask_mut(effects).center_x,
            crate::effects::FilterParam::ShapeMaskCenterY => &mut shape_mask_mut(effects).center_y,
            crate::effects::FilterParam::ShapeMaskWidth => &mut shape_mask_mut(effects).width,
            crate::effects::FilterParam::ShapeMaskHeight => &mut shape_mask_mut(effects).height,
            crate::effects::FilterParam::ShapeMaskFeather => &mut shape_mask_mut(effects).feather,
            crate::effects::FilterParam::LutMix => &mut lut_mut(effects).mix,
        };
        if anim.has_key(clip_relative_frame) {
            anim.remove_key(clip_relative_frame);
        } else {
            anim.set_key(clip_relative_frame, anim.value_at(clip_relative_frame));
        }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EqBand {
    Low,
    Mid,
    High,
}

pub fn set_track_eq(
    project: &mut Project,
    sequence_id: SequenceId,
    track_id: TrackId,
    band: EqBand,
    gain_db: f32,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let track = sequence
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or(EditError::TrackNotFound)?;
        let value = crate::eq::clamp_eq_db(gain_db);
        match band {
            EqBand::Low => track.eq.low = value,
            EqBand::Mid => track.eq.mid = value,
            EqBand::High => track.eq.high = value,
        }
        Ok(())
    })
}

pub fn set_track_eq_low_cut(
    project: &mut Project,
    sequence_id: SequenceId,
    track_id: TrackId,
    enabled: bool,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let track = sequence
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or(EditError::TrackNotFound)?;
        track.eq.low_cut = enabled;
        Ok(())
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompressorParam {
    Threshold,
    Ratio,
    Attack,
    Release,
    Makeup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DuckParam {
    Threshold,
    Amount,
    Attack,
    Release,
}

pub fn set_track_duck_enabled(
    project: &mut Project,
    sequence_id: SequenceId,
    track_id: TrackId,
    enabled: bool,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let track = sequence
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or(EditError::TrackNotFound)?;
        if track.kind != TrackKind::Audio {
            return Err(EditError::WrongTrackKind);
        }
        track.duck.enabled = enabled;
        Ok(())
    })
}

pub fn set_track_duck_source(
    project: &mut Project,
    sequence_id: SequenceId,
    track_id: TrackId,
    source: Option<TrackId>,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        if let Some(source_id) = source {
            if source_id == track_id {
                return Err(EditError::WrongTrackKind);
            }
            let source_track = sequence
                .track(source_id)
                .ok_or(EditError::TrackNotFound)?;
            if source_track.kind != TrackKind::Audio {
                return Err(EditError::WrongTrackKind);
            }
        }
        let track = sequence
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or(EditError::TrackNotFound)?;
        if track.kind != TrackKind::Audio {
            return Err(EditError::WrongTrackKind);
        }
        track.duck.source = source.map(|id| id.0);
        Ok(())
    })
}

pub fn set_track_duck(
    project: &mut Project,
    sequence_id: SequenceId,
    track_id: TrackId,
    param: DuckParam,
    value: f32,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let track = sequence
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or(EditError::TrackNotFound)?;
        if track.kind != TrackKind::Audio {
            return Err(EditError::WrongTrackKind);
        }
        match param {
            DuckParam::Threshold => {
                track.duck.threshold_db = crate::duck::clamp_duck_threshold(value);
            }
            DuckParam::Amount => {
                track.duck.amount_db = crate::duck::clamp_duck_amount(value);
            }
            DuckParam::Attack => {
                track.duck.attack_ms = crate::duck::clamp_duck_attack(value);
            }
            DuckParam::Release => {
                track.duck.release_ms = crate::duck::clamp_duck_release(value);
            }
        }
        Ok(())
    })
}

pub fn set_track_compressor(
    project: &mut Project,
    sequence_id: SequenceId,
    track_id: TrackId,
    param: CompressorParam,
    value: f32,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let track = sequence
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or(EditError::TrackNotFound)?;
        match param {
            CompressorParam::Threshold => {
                track.compressor.threshold_db = crate::compressor::clamp_threshold_db(value);
            }
            CompressorParam::Ratio => {
                track.compressor.ratio = crate::compressor::clamp_ratio(value);
            }
            CompressorParam::Attack => {
                track.compressor.attack_ms = crate::compressor::clamp_attack_ms(value);
            }
            CompressorParam::Release => {
                track.compressor.release_ms = crate::compressor::clamp_release_ms(value);
            }
            CompressorParam::Makeup => {
                track.compressor.makeup_db = crate::compressor::clamp_makeup_db(value);
            }
        }
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
    add_marker_with_color(project, sequence_id, frame, name, LabelColor::Amber)
}

pub fn add_marker_with_color(
    project: &mut Project,
    sequence_id: SequenceId,
    frame: Frame,
    name: impl Into<String>,
    color: LabelColor,
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
            color,
            comment: String::new(),
        });
        sequence.markers.sort_by_key(|m| m.frame.0);
        Ok(())
    })?;
    Ok(id)
}

pub fn update_marker(
    project: &mut Project,
    sequence_id: SequenceId,
    marker_id: MarkerId,
    name: Option<String>,
    color: Option<LabelColor>,
    comment: Option<String>,
    frame: Option<Frame>,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let marker = sequence
            .markers
            .iter_mut()
            .find(|marker| marker.id == marker_id)
            .ok_or(EditError::MarkerNotFound)?;
        if let Some(name) = name {
            marker.name = name;
        }
        if let Some(color) = color {
            marker.color = color;
        }
        if let Some(comment) = comment {
            marker.comment = comment;
        }
        if let Some(frame) = frame {
            marker.frame = frame;
        }
        sequence.markers.sort_by_key(|marker| marker.frame.0);
        Ok(())
    })
}

pub fn delete_marker(
    project: &mut Project,
    sequence_id: SequenceId,
    marker_id: MarkerId,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let index = sequence
            .markers
            .iter()
            .position(|marker| marker.id == marker_id)
            .ok_or(EditError::MarkerNotFound)?;
        sequence.markers.remove(index);
        Ok(())
    })
}

pub fn create_bin(
    project: &mut Project,
    parent: Option<BinId>,
    name: impl Into<String>,
) -> Result<BinId, EditError> {
    if let Some(parent_id) = parent {
        if !project.bins.iter().any(|bin| bin.id == parent_id) {
            return Err(EditError::BinNotFound);
        }
    }
    let id = BinId(project.alloc());
    project.bins.push(Bin {
        id,
        name: name.into(),
        parent,
    });
    Ok(id)
}

pub fn rename_bin(
    project: &mut Project,
    bin_id: BinId,
    name: impl Into<String>,
) -> Result<(), EditError> {
    let bin = project
        .bins
        .iter_mut()
        .find(|bin| bin.id == bin_id)
        .ok_or(EditError::BinNotFound)?;
    bin.name = name.into();
    Ok(())
}

pub fn delete_bin(project: &mut Project, bin_id: BinId) -> Result<(), EditError> {
    if !project.bins.iter().any(|bin| bin.id == bin_id) {
        return Err(EditError::BinNotFound);
    }
    if project.bins.len() <= 1 {
        return Err(EditError::LastBin);
    }
    let parent = project
        .bins
        .iter()
        .find(|bin| bin.id == bin_id)
        .and_then(|bin| bin.parent);
    let destination = parent.or_else(|| {
        project
            .bins
            .iter()
            .find(|bin| bin.id != bin_id)
            .map(|bin| bin.id)
    });
    let Some(destination) = destination else {
        return Err(EditError::LastBin);
    };
    for bin in &mut project.bins {
        if bin.parent == Some(bin_id) {
            bin.parent = parent;
        }
    }
    for media in &mut project.media {
        if media.bin_id == bin_id {
            media.bin_id = destination;
        }
    }
    project.bins.retain(|bin| bin.id != bin_id);
    Ok(())
}

pub fn move_media_to_bin(
    project: &mut Project,
    media_ids: &[MediaId],
    bin_id: BinId,
) -> Result<(), EditError> {
    if !project.bins.iter().any(|bin| bin.id == bin_id) {
        return Err(EditError::BinNotFound);
    }
    for media_id in media_ids {
        let media = project
            .media
            .iter_mut()
            .find(|media| media.id == *media_id)
            .ok_or(EditError::MediaNotFound)?;
        media.bin_id = bin_id;
    }
    Ok(())
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

/// Point `id` at a new file. The media id, bin, and display name stay so
/// timeline clips remain linked. A generated proxy is cleared because it was
/// transcoded from the previous path.
pub fn relink_media(
    project: &mut Project,
    id: MediaId,
    source: &MediaAsset,
) -> Result<(), EditError> {
    let asset = project
        .media
        .iter_mut()
        .find(|media| media.id == id)
        .ok_or(EditError::MediaNotFound)?;
    asset.path = source.path.clone();
    asset.duration = source.duration;
    asset.timebase = source.timebase;
    asset.width = source.width;
    asset.height = source.height;
    asset.video_codec = source.video_codec.clone();
    asset.audio_codec = source.audio_codec.clone();
    asset.audio_channels = source.audio_channels;
    asset.sample_rate = source.sample_rate;
    asset.has_video = source.has_video;
    asset.has_audio = source.has_audio;
    asset.offline = source.offline;
    asset.proxy_path = None;
    Ok(())
}

/// Record generated proxy files on the matching assets.
pub fn attach_proxies(project: &mut Project, links: &[(MediaId, String)]) -> Result<(), EditError> {
    if links.is_empty() {
        return Err(EditError::MediaNotFound);
    }
    for (id, path) in links {
        let asset = project
            .media
            .iter_mut()
            .find(|media| media.id == *id)
            .ok_or(EditError::MediaNotFound)?;
        let trimmed = path.trim();
        asset.proxy_path = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }
    Ok(())
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

/// Resolve source-monitor in/out marks into a media range for insert/overwrite.
///
/// Missing marks fall back to the start or end of the file. Both marks may be
/// unset (full file). Marks are media-timebase frames, the same unit as
/// [`MediaAsset::duration`].
pub fn resolve_source_marks(
    duration: Frame,
    in_point: Option<Frame>,
    out_point: Option<Frame>,
) -> Result<(Frame, Frame), EditError> {
    if duration.0 < 1 {
        return Err(EditError::InvalidDuration);
    }
    let source_in = in_point.unwrap_or(Frame::ZERO).0.max(0).min(duration.0);
    let source_out = out_point
        .unwrap_or(duration)
        .0
        .max(0)
        .min(duration.0);
    if source_out <= source_in {
        return Err(EditError::InvalidDuration);
    }
    Ok((Frame(source_in), Frame(source_out)))
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
        title: None,
        adjustment: false,
        speed: ClipSpeed::normal(),
        multicam: None,
        nested: None,
        track_matte: None,
        hold_frame: None,
    })
}

/// Default freeze duration: two seconds on the sequence timebase.
pub fn default_freeze_duration(timebase: Timebase) -> i64 {
    let frames = (timebase.fps_f64() * 2.0).round() as i64;
    frames.max(1)
}

/// Sample one source frame for the whole clip during preview and export.
pub fn set_clip_hold(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    hold: Frame,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let group = linked_group(sequence, clip_id);
        for id in &group {
            let (track_index, clip_index) =
                sequence.locate_clip(*id).ok_or(EditError::ClipNotFound)?;
            if sequence.tracks[track_index].locked {
                return Err(EditError::TrackLocked);
            }
            let clip = &mut sequence.tracks[track_index].clips[clip_index];
            if clip.is_title() || clip.is_adjustment() {
                return Err(EditError::WrongTrackKind);
            }
            clip.hold_frame = Some(hold);
            clip.speed = ClipSpeed::normal();
            clip.source_in = hold;
            clip.source_out = Frame(hold.0 + 1);
        }
        Ok(())
    })
}

/// Clear a clip hold and restore a one-source-frame span at the held frame.
pub fn clear_clip_hold(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let group = linked_group(sequence, clip_id);
        for id in &group {
            let (track_index, clip_index) =
                sequence.locate_clip(*id).ok_or(EditError::ClipNotFound)?;
            if sequence.tracks[track_index].locked {
                return Err(EditError::TrackLocked);
            }
            let clip = &mut sequence.tracks[track_index].clips[clip_index];
            if clip.hold_frame.is_none() {
                continue;
            }
            let hold = clip.hold_frame.unwrap();
            clip.hold_frame = None;
            clip.source_in = hold;
            clip.source_out = Frame(hold.0 + 1);
        }
        Ok(())
    })
}

/// Turn the clip at `at` into a hold of `duration` sequence frames starting at
/// `at`. When `at` splits a clip, only the right-hand part becomes a hold.
/// Linked clips on targeted tracks take the same hold and duration.
pub fn freeze_at_playhead(
    project: &mut Project,
    sequence_id: SequenceId,
    at: Frame,
    duration: i64,
    track_ids: &[TrackId],
) -> Result<Vec<ClipId>, EditError> {
    if duration < 1 {
        return Err(EditError::InvalidDuration);
    }
    if track_ids.is_empty() {
        return Err(EditError::TrackNotFound);
    }
    let mut frozen = Vec::new();
    map_sequence(project, sequence_id, |sequence, alloc| {
        let allowed: std::collections::HashSet<TrackId> = track_ids.iter().copied().collect();
        let timebase = sequence.timebase;
        let targets: Vec<ClipId> = sequence
            .tracks
            .iter()
            .filter(|track| allowed.contains(&track.id) && !track.locked)
            .filter(|track| track.kind != TrackKind::Caption)
            .filter_map(|track| clip_at_edit(track, at))
            .collect();
        for clip_id in targets {
            if let Some(id) = apply_freeze_to_clip(sequence, clip_id, at, duration, timebase, alloc)? {
                frozen.push(id);
            }
        }
        if frozen.is_empty() {
            return Err(EditError::NotInsideClip);
        }
        Ok(())
    })?;
    Ok(frozen)
}

/// Replace selected clips with a hold of `duration` sequence frames. The held
/// source frame is taken at `at` when the playhead lies inside the clip, or at
/// the clip in-point otherwise.
pub fn freeze_clips_at(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_ids: &[ClipId],
    at: Frame,
    duration: i64,
) -> Result<Vec<ClipId>, EditError> {
    if duration < 1 {
        return Err(EditError::InvalidDuration);
    }
    if clip_ids.is_empty() {
        return Err(EditError::ClipNotFound);
    }
    let mut frozen = Vec::new();
    let mut seen = std::collections::HashSet::new();
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let timebase = sequence.timebase;
        for clip_id in clip_ids {
            if !seen.insert(clip_id) {
                continue;
            }
            if let Some(id) =
                apply_whole_clip_freeze(sequence, *clip_id, at, duration, timebase)?
            {
                frozen.push(id);
            }
        }
        if frozen.is_empty() {
            return Err(EditError::ClipNotFound);
        }
        Ok(())
    })?;
    Ok(frozen)
}

fn clip_at_edit(track: &Track, at: Frame) -> Option<ClipId> {
    if let Some(clip) = track.clips.iter().find(|clip| clip.timeline_in == at) {
        return Some(clip.id);
    }
    track
        .clips
        .iter()
        .find(|clip| clip.covers(at))
        .map(|clip| clip.id)
}

fn apply_whole_clip_freeze(
    sequence: &mut Sequence,
    clip_id: ClipId,
    at: Frame,
    duration: i64,
    timebase: Timebase,
) -> Result<Option<ClipId>, EditError> {
    let primary = sequence
        .clip(clip_id)
        .ok_or(EditError::ClipNotFound)?
        .clone();
    if primary.is_title() || primary.is_adjustment() {
        return Ok(None);
    }
    let group = linked_group(sequence, clip_id);
    let mut applied = None;
    for id in &group {
        let (ti, ci) = sequence.locate_clip(*id).ok_or(EditError::ClipNotFound)?;
        if sequence.tracks[ti].locked {
            return Err(EditError::TrackLocked);
        }
        let clip = &sequence.tracks[ti].clips[ci];
        let sample_at = if clip.covers(at) || clip.timeline_in == at {
            at
        } else {
            clip.timeline_in
        };
        let hold = source_frame_at(clip, sample_at, timebase);
        let old_out = clip.timeline_out.0;
        let new_out = clip.timeline_in.0 + duration;
        let clip = &mut sequence.tracks[ti].clips[ci];
        clip.hold_frame = Some(hold);
        clip.speed = ClipSpeed::normal();
        clip.source_in = hold;
        clip.source_out = Frame(hold.0 + 1);
        clip.timeline_out = Frame(new_out);
        let delta = new_out - old_out;
        if delta != 0 {
            ripple_downstream_except(
                sequence,
                sequence.tracks[ti].id,
                old_out,
                delta,
                *id,
                &group,
            )?;
        }
        applied = Some(*id);
    }
    Ok(applied)
}

fn apply_freeze_to_clip(
    sequence: &mut Sequence,
    clip_id: ClipId,
    at: Frame,
    duration: i64,
    timebase: Timebase,
    alloc: &mut dyn FnMut() -> u64,
) -> Result<Option<ClipId>, EditError> {
    let primary = sequence
        .clip(clip_id)
        .ok_or(EditError::ClipNotFound)?
        .clone();
    if primary.is_title() || primary.is_adjustment() {
        return Ok(None);
    }
    if !primary.covers(at) && primary.timeline_in != at {
        return Ok(None);
    }
    let group = linked_group(sequence, clip_id);
    let mut pairs = Vec::new();
    for id in &group {
        let partner = sequence.clip(*id).ok_or(EditError::ClipNotFound)?;
        if !partner.covers(at) && partner.timeline_in != at {
            continue;
        }
        let target = if at.0 > partner.timeline_in.0 && at.0 < partner.timeline_out.0 {
            split_clip(sequence, *id, at, alloc)?
        } else {
            *id
        };
        pairs.push((*id, target));
    }
    relink_splits(sequence, &pairs);
    let mut applied = None;
    for (_, target) in pairs {
        let (ti, ci) = sequence.locate_clip(target).ok_or(EditError::ClipNotFound)?;
        if sequence.tracks[ti].locked {
            return Err(EditError::TrackLocked);
        }
        let hold = source_frame_at(&sequence.tracks[ti].clips[ci], at, timebase);
        let clip = &mut sequence.tracks[ti].clips[ci];
        let freeze_start = at.0.max(clip.timeline_in.0);
        let old_out = clip.timeline_out.0;
        let new_out = freeze_start + duration;
        clip.hold_frame = Some(hold);
        clip.speed = ClipSpeed::normal();
        clip.source_in = hold;
        clip.source_out = Frame(hold.0 + 1);
        clip.timeline_out = Frame(new_out);
        let delta = new_out - old_out;
        if delta != 0 {
            ripple_downstream_except(
                sequence,
                sequence.tracks[ti].id,
                old_out,
                delta,
                target,
                &group,
            )?;
        }
        applied = Some(target);
    }
    Ok(applied)
}

/// Place a five-second title generator at `at` on the highest video track that
/// has room. If every video track is busy, a new video track is added on top.
pub fn add_title(
    project: &mut Project,
    sequence_id: SequenceId,
    at: Frame,
    text: &str,
) -> Result<ClipId, EditError> {
    if at.0 < 0 {
        return Err(EditError::OutOfRange);
    }
    let text = {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            "Title"
        } else {
            trimmed
        }
    };
    let timebase = project
        .sequence(sequence_id)
        .ok_or(EditError::SequenceNotFound)?
        .timebase;
    let mut duration = (timebase.fps_f64() * 5.0).round() as i64;
    if duration < 1 {
        duration = 1;
    }
    let end = at.0.saturating_add(duration);
    let track_id = ensure_title_track(project, sequence_id, at.0, end)?;
    let id = ClipId(project.alloc());
    let clip = Clip::generator(id.0, at.0, end, timebase, Title::new(text));
    overwrite_clips(project, sequence_id, vec![(track_id, clip)])?;
    Ok(id)
}

/// Place a five-second adjustment layer at `at` on the highest video track that
/// has room. If every video track is busy, a new video track is added on top.
pub fn add_adjustment_layer(
    project: &mut Project,
    sequence_id: SequenceId,
    at: Frame,
    name: &str,
) -> Result<ClipId, EditError> {
    if at.0 < 0 {
        return Err(EditError::OutOfRange);
    }
    let name = {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            "Adjustment"
        } else {
            trimmed
        }
    };
    let timebase = project
        .sequence(sequence_id)
        .ok_or(EditError::SequenceNotFound)?
        .timebase;
    let mut duration = (timebase.fps_f64() * 5.0).round() as i64;
    if duration < 1 {
        duration = 1;
    }
    let end = at.0.saturating_add(duration);
    let track_id = ensure_title_track(project, sequence_id, at.0, end)?;
    let id = ClipId(project.alloc());
    let clip = Clip::adjustment_layer(id.0, at.0, end, timebase, name);
    overwrite_clips(project, sequence_id, vec![(track_id, clip)])?;
    Ok(id)
}

/// Replace the generator payload on a title clip and keep the timeline name
/// in sync with the first line.
pub fn set_clip_title(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    title: Title,
) -> Result<(), EditError> {
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        let clip = &mut sequence.tracks[ti].clips[ci];
        if clip.title.is_none() {
            return Err(EditError::NotATitle);
        }
        let title = title.sanitized();
        clip.name = title.timeline_name();
        clip.title = Some(title);
        Ok(())
    })
}

fn ensure_title_track(
    project: &mut Project,
    sequence_id: SequenceId,
    start: i64,
    end: i64,
) -> Result<TrackId, EditError> {
    let sequence = project
        .sequence(sequence_id)
        .ok_or(EditError::SequenceNotFound)?;
    let videos: Vec<(TrackId, bool)> = sequence
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video && !track.locked)
        .map(|track| {
            let busy = track
                .clips
                .iter()
                .any(|clip| clip.timeline_in.0 < end && clip.timeline_out.0 > start);
            (track.id, busy)
        })
        .collect();
    if let Some((id, _)) = videos.iter().rev().find(|(_, busy)| !*busy) {
        return Ok(*id);
    }
    let count = sequence
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
        .count();
    let id = TrackId(project.alloc());
    let track = crate::model::Track::new(id, TrackKind::Video, format!("V{}", count + 1));
    let sequence = project
        .sequence_mut(sequence_id)
        .ok_or(EditError::SequenceNotFound)?;
    let insert_at = sequence
        .tracks
        .iter()
        .rposition(|item| item.kind == TrackKind::Video)
        .map(|index| index + 1)
        .unwrap_or(0);
    sequence.tracks.insert(insert_at, track);
    Ok(id)
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
    // A retimed clip consumes source at its average rate, so a trim of one
    // timeline frame moves as many source frames as playback would.
    if !clip.speed.is_identity() {
        let rate = clip.speed.average_rate(clip.duration().max(1));
        return crate::speed::source_frames_for_rate(
            timeline_delta,
            rate,
            sequence_timebase,
            clip.media_timebase,
        );
    }
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
///
/// A 100% forward clip keeps the integer timebase mapping. Any other speed,
/// including a ramp or reverse, samples the integral of the rate curve.
pub fn source_frame_at(clip: &Clip, timeline_frame: Frame, sequence_timebase: Timebase) -> Frame {
    if let Some(hold) = clip.hold_frame {
        return hold;
    }
    if !clip.speed.is_identity() {
        return crate::speed::mapped_source_frame(clip, timeline_frame, sequence_timebase);
    }
    let delta = timeline_frame.0 - clip.timeline_in.0;
    Frame(clip.source_in.0 + source_delta(clip, delta, sequence_timebase))
}

/// Sequence frame where `clip` shows `source_frame`. Inverse of [`source_frame_at`]
/// for the clip's current mapping. Rounding can drift when the rates are not equal.
/// A retimed clip is inverted by searching [`source_frame_at`], so speed math stays
/// in the speed module.
pub fn timeline_frame_at_source(
    clip: &Clip,
    source_frame: Frame,
    sequence_timebase: Timebase,
) -> Frame {
    if clip.hold_frame.is_some() {
        return clip.timeline_in;
    }
    if !clip.speed.is_identity() {
        return timeline_frame_for_retimed_source(clip, source_frame, sequence_timebase);
    }
    let delta = source_frame.0 - clip.source_in.0;
    Frame(clip.timeline_in.0 + inverse_source_delta(clip, delta, sequence_timebase))
}

fn inverse_source_delta(clip: &Clip, source_delta_frames: i64, sequence_timebase: Timebase) -> i64 {
    let timeline_span = clip.duration();
    let source_span = clip.source_duration();
    if timeline_span > 0 && source_span > 0 {
        let mapped = convert_frames(timeline_span, sequence_timebase, clip.media_timebase);
        if mapped == source_span {
            return convert_frames(source_delta_frames, clip.media_timebase, sequence_timebase);
        }
        return mul_div_round(source_delta_frames, timeline_span, source_span);
    }
    convert_frames(source_delta_frames, clip.media_timebase, sequence_timebase)
}

fn timeline_frame_for_retimed_source(
    clip: &Clip,
    source_frame: Frame,
    sequence_timebase: Timebase,
) -> Frame {
    let start = clip.timeline_in.0;
    let end = clip.timeline_out.0.max(start);
    if end <= start {
        return clip.timeline_in;
    }
    let mut lo = start;
    let mut hi = end;
    let reverse = clip.speed.reverse;
    while lo + 1 < hi {
        let mid = lo + (hi - lo) / 2;
        let mapped = source_frame_at(clip, Frame(mid), sequence_timebase).0;
        let before = if reverse {
            mapped > source_frame.0
        } else {
            mapped < source_frame.0
        };
        if before {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let at_lo = source_frame_at(clip, Frame(lo), sequence_timebase).0;
    let hi_frame = hi.saturating_sub(1).max(lo);
    let at_hi = source_frame_at(clip, Frame(hi_frame), sequence_timebase).0;
    let pick = if (at_hi - source_frame.0).abs() < (at_lo - source_frame.0).abs() {
        hi_frame
    } else {
        lo
    };
    Frame(pick)
}

/// Set playback speed on `clip_id` and every clip linked to it.
///
/// The timeline duration becomes the length that consumes the same source
/// range at the new average rate. Later clips on the clip's track and on
/// sync-locked tracks shift by that change. Linked clips take the same speed
/// and the same duration so picture and sound stay aligned. Reverse does not
/// change the duration.
pub fn set_clip_speed(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    speed: ClipSpeed,
) -> Result<(), EditError> {
    let speed = speed.sanitized();
    map_sequence(project, sequence_id, |sequence, _alloc| {
        let (ti, ci) = sequence
            .locate_clip(clip_id)
            .ok_or(EditError::ClipNotFound)?;
        if sequence.tracks[ti].locked {
            return Err(EditError::TrackLocked);
        }
        let group = linked_group(sequence, clip_id);
        for id in &group {
            let (track_index, _) = sequence.locate_clip(*id).ok_or(EditError::ClipNotFound)?;
            if sequence.tracks[track_index].locked {
                return Err(EditError::TrackLocked);
            }
        }
        let primary = sequence.tracks[ti].clips[ci].clone();
        let old_out = primary.timeline_out.0;
        let rate = speed.average_rate(primary.duration().max(1));
        let new_span = crate::speed::timeline_span_for_speed(
            primary.source_duration().max(1),
            rate,
            primary.media_timebase,
            sequence.timebase,
        );
        let delta = new_span - primary.duration();
        for id in &group {
            let (track_index, clip_index) =
                sequence.locate_clip(*id).ok_or(EditError::ClipNotFound)?;
            let clip = &mut sequence.tracks[track_index].clips[clip_index];
            clip.speed = speed.clone();
            clip.timeline_out = Frame(clip.timeline_in.0 + new_span);
        }
        if delta != 0 {
            let origin = sequence.tracks[ti].id;
            ripple_downstream_except(sequence, origin, old_out, delta, clip_id, &group)?;
        }
        Ok(())
    })
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

fn clips_containing_on_tracks(
    sequence: &Sequence,
    allowed: &std::collections::HashSet<TrackId>,
    at: Frame,
) -> Vec<ClipId> {
    let mut ids = Vec::new();
    for track in &sequence.tracks {
        if !allowed.contains(&track.id) || track.locked {
            continue;
        }
        for clip in &track.clips {
            if clip.contains_frame(at) {
                ids.push(clip.id);
            }
        }
    }
    ids
}

fn expand_split_targets(sequence: &Sequence, seeds: &[ClipId], at: Frame) -> Vec<ClipId> {
    let mut targets = seeds.to_vec();
    for id in seeds {
        if let Some(clip) = sequence.clip(*id) {
            for linked in &clip.linked {
                if let Some(partner) = sequence.clip(*linked) {
                    if partner.contains_frame(at) {
                        targets.push(*linked);
                    }
                }
            }
        }
    }
    targets.sort_unstable();
    targets.dedup();
    targets
}

fn trim_in_sequence(
    sequence: &mut Sequence,
    clip_id: ClipId,
    edge: TrimEdge,
    delta: i64,
) -> Result<(), EditError> {
    if delta == 0 {
        return Ok(());
    }
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
}

fn ripple_trim_in_sequence(
    sequence: &mut Sequence,
    clip_id: ClipId,
    edge: TrimEdge,
    delta: i64,
) -> Result<(), EditError> {
    if delta == 0 {
        return Ok(());
    }
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
            sequence.tracks[ti].clips[ci].source_in = Frame(new_source);
            sequence.tracks[ti].clips[ci].timeline_out = Frame(new_out);
        }
    }
    ripple_downstream_except(sequence, origin, old_out, delta, clip_id, &[])?;
    Ok(())
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

fn ripple_downstream_except(
    sequence: &mut Sequence,
    origin: TrackId,
    at: i64,
    delta: i64,
    skip: ClipId,
    except: &[ClipId],
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
            if clip.id == skip || except.contains(&clip.id) {
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
    fn duck_settings_round_trip_and_reject_a_self_source() {
        let mut sequence = Sequence::new(SequenceId(1), "Duck", 1920, 1080, Timebase::fps_24());
        sequence.add_track(TrackId(2), TrackKind::Audio, "Music");
        sequence.add_track(TrackId(3), TrackKind::Audio, "Dial");
        sequence.add_track(TrackId(4), TrackKind::Video, "V1");
        let mut project = project_with(sequence);
        let seq = project.active_sequence.unwrap();
        set_track_duck_enabled(&mut project, seq, TrackId(2), true).unwrap();
        set_track_duck_source(&mut project, seq, TrackId(2), Some(TrackId(3))).unwrap();
        set_track_duck(&mut project, seq, TrackId(2), DuckParam::Threshold, -30.0).unwrap();
        set_track_duck(&mut project, seq, TrackId(2), DuckParam::Amount, 15.0).unwrap();
        set_track_duck(&mut project, seq, TrackId(2), DuckParam::Attack, 20.0).unwrap();
        set_track_duck(&mut project, seq, TrackId(2), DuckParam::Release, 400.0).unwrap();
        assert_eq!(
            set_track_duck_source(&mut project, seq, TrackId(2), Some(TrackId(2))),
            Err(EditError::WrongTrackKind)
        );
        assert_eq!(
            set_track_duck_source(&mut project, seq, TrackId(2), Some(TrackId(4))),
            Err(EditError::WrongTrackKind)
        );
        let track = &project.active().unwrap().tracks[0];
        assert!(track.duck.enabled);
        assert_eq!(track.duck.source, Some(3));
        assert!((track.duck.threshold_db + 30.0).abs() < 1.0e-4);
        assert!((track.duck.amount_db - 15.0).abs() < 1.0e-4);
        assert!((track.duck.attack_ms - 20.0).abs() < 1.0e-4);
        assert!((track.duck.release_ms - 400.0).abs() < 1.0e-4);
        let json = serde_json::to_string(project.active().unwrap()).unwrap();
        assert!(json.contains("\"duck\""), "{json}");
        let loaded: Sequence = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.tracks[0].duck, project.active().unwrap().tracks[0].duck);
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        for track in value["tracks"].as_array_mut().unwrap() {
            track.as_object_mut().unwrap().remove("duck");
        }
        let loaded: Sequence = serde_json::from_str(&value.to_string()).unwrap();
        assert!(loaded.tracks[0].duck.is_bypass());
        let fresh = crate::model::Track::new(TrackId(1), TrackKind::Audio, "A1");
        assert!(!serde_json::to_string(&fresh).unwrap().contains("duck"));
    }

    #[test]
    fn resolve_source_marks_defaults_to_full_file() {
        assert_eq!(
            resolve_source_marks(Frame(120), None, None).unwrap(),
            (Frame(0), Frame(120))
        );
        assert_eq!(
            resolve_source_marks(Frame(120), Some(Frame(24)), None).unwrap(),
            (Frame(24), Frame(120))
        );
        assert_eq!(
            resolve_source_marks(Frame(120), None, Some(Frame(48))).unwrap(),
            (Frame(0), Frame(48))
        );
        assert_eq!(
            resolve_source_marks(Frame(120), Some(Frame(10)), Some(Frame(40))).unwrap(),
            (Frame(10), Frame(40))
        );
        assert_eq!(
            resolve_source_marks(Frame(120), Some(Frame(40)), Some(Frame(40))),
            Err(EditError::InvalidDuration)
        );
        assert_eq!(
            resolve_source_marks(Frame(120), Some(Frame(80)), Some(Frame(40))),
            Err(EditError::InvalidDuration)
        );
    }

    #[test]
    fn clip_from_media_uses_source_marks_for_timeline_span() {
        let media = MediaAsset {
            id: MediaId(1),
            bin_id: crate::model::BinId(0),
            name: "clip".into(),
            path: "clip.mp4".into(),
            duration: Frame(96),
            timebase: Timebase::fps_24(),
            width: Some(1920),
            height: Some(1080),
            video_codec: None,
            audio_codec: None,
            audio_channels: None,
            sample_rate: None,
            has_video: true,
            has_audio: false,
            offline: false,
            proxy_path: None,
        };
        let clip = clip_from_media(
            ClipId(1),
            &media,
            Timebase::fps_24(),
            Frame(10),
            Frame(24),
            Frame(72),
            "clip",
        )
        .unwrap();
        assert_eq!(clip.timeline_in.0, 10);
        assert_eq!(clip.timeline_out.0, 58);
        assert_eq!(clip.source_in.0, 24);
        assert_eq!(clip.source_out.0, 72);
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

    #[test]
    fn set_speed_ripples_duration_and_linked_audio_keeps_the_same_span() {
        let mut sequence = Sequence::new(SequenceId(1), "T", 1920, 1080, Timebase::fps_24());
        let video = sequence.add_track(TrackId(2), TrackKind::Video, "V1");
        let audio = sequence.add_track(TrackId(3), TrackKind::Audio, "A1");
        sequence.tracks[0].sync_lock = true;
        sequence.tracks[1].sync_lock = true;
        let mut picture = Clip::basic(1, 0, 48);
        let mut voice = Clip::basic(2, 0, 48);
        link_clips(&mut picture, &mut voice);
        sequence.tracks[0].clips = vec![picture, Clip::basic(3, 48, 96)];
        sequence.tracks[1].clips = vec![voice];
        let mut project = project_with(sequence);
        set_clip_speed(
            &mut project,
            SequenceId(1),
            ClipId(1),
            ClipSpeed::constant(2.0, false),
        )
        .unwrap();
        let seq = project.sequence(SequenceId(1)).unwrap();
        let picture = seq.clip(ClipId(1)).unwrap();
        assert_eq!((picture.timeline_in.0, picture.timeline_out.0), (0, 24));
        assert_eq!((picture.source_in.0, picture.source_out.0), (0, 48));
        assert!((picture.speed.start_rate() - 2.0).abs() < 1.0e-6);
        assert_eq!(source_frame_at(picture, Frame(0), Timebase::fps_24()).0, 0);
        assert_eq!(
            source_frame_at(picture, Frame(12), Timebase::fps_24()).0,
            24
        );
        let voice = seq.clip(ClipId(2)).unwrap();
        assert_eq!((voice.timeline_in.0, voice.timeline_out.0), (0, 24));
        assert!(voice.speed.mutes_audio());
        assert_eq!(source_frame_at(voice, Frame(12), Timebase::fps_24()).0, 24);
        let later = seq.clip(ClipId(3)).unwrap();
        assert_eq!((later.timeline_in.0, later.timeline_out.0), (24, 72));
        assert_eq!(later.source_in.0, 0);

        set_clip_speed(
            &mut project,
            SequenceId(1),
            ClipId(1),
            ClipSpeed::ramp(1.0, 3.0, false),
        )
        .unwrap();
        let seq = project.sequence(SequenceId(1)).unwrap();
        let picture = seq.clip(ClipId(1)).unwrap();
        assert_eq!(picture.duration(), 24);
        assert_eq!(
            source_frame_at(picture, Frame(12), Timebase::fps_24()).0,
            18
        );
        let later = seq.clip(ClipId(3)).unwrap();
        assert_eq!((later.timeline_in.0, later.timeline_out.0), (24, 72));

        set_clip_speed(
            &mut project,
            SequenceId(1),
            ClipId(1),
            ClipSpeed::constant(1.0, true),
        )
        .unwrap();
        let seq = project.sequence(SequenceId(1)).unwrap();
        let picture = seq.clip(ClipId(1)).unwrap();
        assert_eq!((picture.timeline_in.0, picture.timeline_out.0), (0, 48));
        assert_eq!(source_frame_at(picture, Frame(0), Timebase::fps_24()).0, 47);
        assert_eq!(source_frame_at(picture, Frame(1), Timebase::fps_24()).0, 46);
        let later = seq.clip(ClipId(3)).unwrap();
        assert_eq!((later.timeline_in.0, later.timeline_out.0), (48, 96));
        let _ = (video, audio);
    }

    #[test]
    fn hold_frame_samples_one_source_frame_for_the_whole_clip() {
        let (mut sequence, _track) = video_sequence();
        sequence.tracks[0].clips = vec![Clip::basic(1, 0, 48)];
        let mut project = project_with(sequence);
        set_clip_hold(&mut project, SequenceId(1), ClipId(1), Frame(12)).unwrap();
        let clip = project.sequence(SequenceId(1)).unwrap().clip(ClipId(1)).unwrap();
        assert_eq!(clip.hold_frame, Some(Frame(12)));
        assert!(clip.mutes_audio());
        assert_eq!(source_frame_at(clip, Frame(0), Timebase::fps_24()).0, 12);
        assert_eq!(source_frame_at(clip, Frame(24), Timebase::fps_24()).0, 12);
        let json = serde_json::to_string(clip).unwrap();
        let loaded: Clip = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.hold_frame, Some(Frame(12)));
    }

    #[test]
    fn freeze_at_playhead_splits_and_extends_with_hold() {
        let (mut sequence, track) = video_sequence();
        sequence.tracks[0].clips = vec![Clip::basic(1, 0, 48), Clip::basic(2, 48, 96)];
        let mut project = project_with(sequence);
        let frozen = freeze_at_playhead(
            &mut project,
            SequenceId(1),
            Frame(24),
            12,
            &[track],
        )
        .unwrap();
        assert_eq!(frozen.len(), 1);
        let seq = project.sequence(SequenceId(1)).unwrap();
        let left = seq.clip(ClipId(1)).unwrap();
        let hold = seq.clip(frozen[0]).unwrap();
        let later = seq.clip(ClipId(2)).unwrap();
        assert_eq!((left.timeline_in.0, left.timeline_out.0), (0, 24));
        assert_eq!((hold.timeline_in.0, hold.timeline_out.0), (24, 36));
        assert_eq!(hold.hold_frame, Some(Frame(24)));
        assert_eq!(source_frame_at(hold, Frame(30), Timebase::fps_24()).0, 24);
        assert_eq!((later.timeline_in.0, later.timeline_out.0), (36, 84));
    }

    #[test]
    fn freeze_clips_at_replaces_selection_with_hold() {
        let (mut sequence, _track) = video_sequence();
        sequence.tracks[0].clips = vec![Clip::basic(1, 0, 48)];
        let mut project = project_with(sequence);
        let frozen =
            freeze_clips_at(&mut project, SequenceId(1), &[ClipId(1)], Frame(10), 8).unwrap();
        let hold = project
            .sequence(SequenceId(1))
            .unwrap()
            .clip(frozen[0])
            .unwrap();
        assert_eq!((hold.timeline_in.0, hold.timeline_out.0), (0, 8));
        assert_eq!(hold.hold_frame, Some(Frame(10)));
        assert_eq!(source_frame_at(hold, Frame(4), Timebase::fps_24()).0, 10);
    }

    #[test]
    fn trim_on_a_fast_clip_consumes_source_at_that_speed() {
        let (mut sequence, track) = video_sequence();
        sequence.tracks[0].clips = vec![Clip::basic(1, 0, 48).with_handles(0, 24)];
        let mut project = project_with(sequence);
        set_clip_speed(
            &mut project,
            SequenceId(1),
            ClipId(1),
            ClipSpeed::constant(2.0, false),
        )
        .unwrap();
        trim(&mut project, SequenceId(1), ClipId(1), TrimEdge::Tail, 2).unwrap();
        let clip = project
            .sequence(SequenceId(1))
            .unwrap()
            .track(track)
            .unwrap()
            .clips[0]
            .clone();
        assert_eq!((clip.timeline_in.0, clip.timeline_out.0), (0, 26));
        assert_eq!((clip.source_in.0, clip.source_out.0), (0, 52));
        assert_eq!(source_frame_at(&clip, Frame(25), Timebase::fps_24()).0, 50);
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
    fn ripple_trim_prev_to_playhead_inside_clip() {
        let (mut sequence, track) = video_sequence();
        sequence.tracks[0].clips = vec![Clip::basic(1, 0, 200).with_handles(0, 100)];
        let mut project = project_with(sequence);
        ripple_trim_prev_to_playhead(&mut project, SequenceId(1), Frame(80), &[track]).unwrap();
        let seq = project.active().unwrap();
        let clips = &seq.track(track).unwrap().clips;
        assert_eq!(clips.len(), 1);
        assert_eq!((clips[0].timeline_in.0, clips[0].timeline_out.0), (80, 200));
        assert_eq!(clips[0].source_in.0, 80);
    }

    #[test]
    fn ripple_trim_next_to_playhead_inside_clip() {
        let (mut sequence, track) = video_sequence();
        sequence.tracks[0].clips = vec![Clip::basic(1, 0, 200).with_handles(0, 100)];
        let mut project = project_with(sequence);
        ripple_trim_next_to_playhead(&mut project, SequenceId(1), Frame(120), &[track]).unwrap();
        let seq = project.active().unwrap();
        let clips = &seq.track(track).unwrap().clips;
        assert_eq!(clips.len(), 1);
        assert_eq!((clips[0].timeline_in.0, clips[0].timeline_out.0), (0, 120));
        assert_eq!(clips[0].source_out.0, 120);
    }

    #[test]
    fn ripple_trim_prev_to_playhead_extends_across_gap() {
        let (mut sequence, track) = video_sequence();
        sequence.tracks[0].clips = vec![
            Clip::basic(1, 0, 100).with_handles(0, 40),
            Clip::basic(2, 150, 250).with_handles(0, 40),
        ];
        let mut project = project_with(sequence);
        ripple_trim_prev_to_playhead(&mut project, SequenceId(1), Frame(125), &[track]).unwrap();
        let seq = project.active().unwrap();
        let a = seq.clip(ClipId(1)).unwrap();
        assert_eq!((a.timeline_in.0, a.timeline_out.0), (0, 125));
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
            proxy_path: None,
        };
        let id = import_media(&mut project, asset);
        let json = project.to_json_pretty().unwrap();
        let mut loaded = Project::from_json(&json).unwrap();
        let media = loaded.media(id).unwrap();
        assert_eq!(media.path, "/home/editor/footage/interview.mp4");
        assert_eq!(media.name, "interview.mp4");
        assert_eq!(media.bin_id, bin);
        assert!(media.has_video && media.has_audio);
        assert!(!media.offline);
        assert_eq!(media.duration, Frame(240));
        assert!(media.proxy_path.is_none());

        attach_proxies(&mut loaded, &[(id, "/cache/interview-proxy.mp4".into())]).unwrap();
        assert_eq!(
            loaded.media(id).unwrap().proxy_path.as_deref(),
            Some("/cache/interview-proxy.mp4")
        );
        let json = loaded.to_json_pretty().unwrap();
        let mut loaded = Project::from_json(&json).unwrap();
        assert_eq!(
            loaded.media(id).unwrap().proxy_path.as_deref(),
            Some("/cache/interview-proxy.mp4")
        );

        let mut replacement = loaded.media(id).unwrap().clone();
        replacement.path = "/restore/interview.mp4".into();
        replacement.offline = true;
        relink_media(&mut loaded, id, &replacement).unwrap();
        let media = loaded.media(id).unwrap();
        assert_eq!(media.path, "/restore/interview.mp4");
        assert!(media.offline);
        assert!(media.proxy_path.is_none());
        assert_eq!(media.name, "interview.mp4");
        assert_eq!(
            relink_media(&mut loaded, MediaId(99), &replacement),
            Err(EditError::MediaNotFound)
        );
    }

    #[test]
    fn adjustment_round_trips_and_old_clips_stay_unadjusted() {
        let clip = Clip::basic(1, 0, 24);
        let json = serde_json::to_string(&clip).unwrap();
        assert!(!json.contains("adjustment"));
        let loaded: Clip = serde_json::from_str(&json).unwrap();
        assert!(!loaded.adjustment);

        let adjusted = Clip::adjustment_layer(7, 0, 48, Timebase::fps_24(), "Warm wash");
        let json = serde_json::to_string(&adjusted).unwrap();
        assert!(json.contains("\"adjustment\":true"));
        let loaded: Clip = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, adjusted);
        assert!(loaded.is_adjustment());
        assert_eq!(loaded.name, "Warm wash");
    }

    #[test]
    fn add_adjustment_layer_uses_a_free_video_track() {
        let (sequence, track) = video_sequence();
        let mut project = project_with(sequence);
        sequence_busy(&mut project, track);
        let id =
            add_adjustment_layer(&mut project, SequenceId(1), Frame(0), "  Grade  ").unwrap();
        let clip = project.active().unwrap().clip(id).unwrap();
        assert!(clip.is_adjustment());
        assert_eq!(clip.name, "Grade");
        assert_eq!(clip.timeline_in, Frame(0));
        assert!(clip.duration() >= 24);
        assert_ne!(
            project.active().unwrap().locate_clip(id).unwrap().0,
            0,
            "a busy V1 should not be overwritten"
        );
        let json = project.to_json_pretty().unwrap();
        let loaded = Project::from_json(&json).unwrap();
        assert!(loaded.active().unwrap().clip(id).unwrap().is_adjustment());
    }

    #[test]
    fn title_round_trips_and_old_clips_stay_untitled() {
        let clip = Clip::basic(1, 0, 24);
        let json = serde_json::to_string(&clip).unwrap();
        assert!(!json.contains("title"));
        let loaded: Clip = serde_json::from_str(&json).unwrap();
        assert!(loaded.title.is_none());

        let mut titled = Clip::generator(7, 0, 48, Timebase::fps_24(), Title::new("Hello\nthere"));
        titled.title.as_mut().unwrap().align = crate::model::TextAlign::Left;
        let json = serde_json::to_string(&titled).unwrap();
        let loaded: Clip = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, titled);
        assert_eq!(loaded.name, "Hello");
        assert_eq!(loaded.title.unwrap().text, "Hello\nthere");
    }

    #[test]
    fn add_title_uses_a_free_video_track_and_can_be_edited() {
        let (sequence, track) = video_sequence();
        let mut project = project_with(sequence);
        sequence_busy(&mut project, track);
        let id = add_title(&mut project, SequenceId(1), Frame(0), "  Lower third  ").unwrap();
        let clip = project.active().unwrap().clip(id).unwrap();
        assert!(clip.is_title());
        assert_eq!(clip.name, "Lower third");
        assert_eq!(clip.timeline_in, Frame(0));
        assert!(clip.duration() >= 24);
        assert_ne!(
            project.active().unwrap().locate_clip(id).unwrap().0,
            0,
            "a busy V1 should not be overwritten"
        );
        let mut title = clip.title.clone().unwrap();
        title.text = "Updated".into();
        title.font_size = 0.09;
        title.x = 0.25;
        set_clip_title(&mut project, SequenceId(1), id, title).unwrap();
        let clip = project.active().unwrap().clip(id).unwrap();
        assert_eq!(clip.name, "Updated");
        assert!((clip.title.as_ref().unwrap().font_size - 0.09).abs() < 1.0e-5);
        let picture = project.active().unwrap().clip(ClipId(1));
        assert!(picture.is_some(), "the picture clip stays");
        let json = project.to_json_pretty().unwrap();
        let loaded = Project::from_json(&json).unwrap();
        assert_eq!(
            loaded.active().unwrap().clip(id).unwrap().title,
            project.active().unwrap().clip(id).unwrap().title
        );
        assert!(
            set_clip_title(&mut project, SequenceId(1), ClipId(1), Title::new("nope")).is_err()
        );
    }

    fn sequence_busy(project: &mut Project, track: TrackId) {
        let clip = Clip::basic(1, 0, 200);
        overwrite_clips(project, SequenceId(1), vec![(track, clip)]).unwrap();
    }

    #[test]
    fn bins_and_markers_round_trip_in_project_json() {
        use crate::model::{Bin, BinId, MediaAsset, MediaId, Sequence, SequenceId};

        let mut project = Project::new("bins");
        let master = BinId(project.alloc());
        project.bins.push(Bin {
            id: master,
            name: "Master".into(),
            parent: None,
        });
        let interviews = create_bin(&mut project, Some(master), "Interviews").unwrap();
        let asset = MediaAsset {
            id: MediaId(0),
            bin_id: BinId(0),
            name: "clip.mp4".into(),
            path: "/tmp/clip.mp4".into(),
            duration: Frame(120),
            timebase: Timebase::fps_24(),
            width: Some(1920),
            height: Some(1080),
            video_codec: None,
            audio_codec: None,
            audio_channels: None,
            sample_rate: None,
            has_video: true,
            has_audio: true,
            offline: false,
            proxy_path: None,
        };
        let media_id = import_media(&mut project, asset);
        move_media_to_bin(&mut project, &[media_id], interviews).unwrap();
        rename_bin(&mut project, interviews, "Interview selects").unwrap();

        let sequence = Sequence::new(SequenceId(1), "Main", 1920, 1080, Timebase::fps_24());
        project.sequences.push(sequence);
        project.active_sequence = Some(SequenceId(1));

        let marker_id =
            add_marker_with_color(&mut project, SequenceId(1), Frame(48), "Beat", LabelColor::Teal)
                .unwrap();
        update_marker(
            &mut project,
            SequenceId(1),
            marker_id,
            Some("Downbeat".into()),
            Some(LabelColor::Rose),
            Some("First hit.".into()),
            None,
        )
        .unwrap();

        let json = project.to_json_pretty().unwrap();
        let mut loaded = Project::from_json(&json).unwrap();
        assert_eq!(loaded.media(media_id).unwrap().bin_id, interviews);
        assert_eq!(
            loaded
                .bins
                .iter()
                .find(|bin| bin.id == interviews)
                .unwrap()
                .name,
            "Interview selects"
        );
        let marker = loaded
            .active()
            .unwrap()
            .markers
            .iter()
            .find(|marker| marker.id == marker_id)
            .unwrap();
        assert_eq!(marker.name, "Downbeat");
        assert_eq!(marker.color, LabelColor::Rose);
        assert_eq!(marker.comment, "First hit.");

        delete_marker(&mut loaded, SequenceId(1), marker_id).unwrap();
        assert!(loaded.active().unwrap().markers.is_empty());
        delete_bin(&mut loaded, interviews).unwrap();
        assert_eq!(loaded.media(media_id).unwrap().bin_id, master);
    }

    #[test]
    fn delete_bin_reparents_children_and_moves_media() {
        let mut project = Project::new("tree");
        let master = create_bin(&mut project, None, "Master").unwrap();
        let child = create_bin(&mut project, Some(master), "Child").unwrap();
        let grandchild = create_bin(&mut project, Some(child), "Grandchild").unwrap();
        let asset = MediaAsset {
            id: MediaId(0),
            bin_id: BinId(0),
            name: "a.mov".into(),
            path: "/tmp/a.mov".into(),
            duration: Frame(24),
            timebase: Timebase::fps_24(),
            width: None,
            height: None,
            video_codec: None,
            audio_codec: None,
            audio_channels: None,
            sample_rate: None,
            has_video: true,
            has_audio: false,
            offline: false,
            proxy_path: None,
        };
        let media_id = import_media(&mut project, asset);
        move_media_to_bin(&mut project, &[media_id], child).unwrap();

        delete_bin(&mut project, child).unwrap();
        assert!(project.bins.iter().any(|bin| bin.id == grandchild));
        assert_eq!(
            project
                .bins
                .iter()
                .find(|bin| bin.id == grandchild)
                .unwrap()
                .parent,
            Some(master)
        );
        assert_eq!(project.media(media_id).unwrap().bin_id, master);
        delete_bin(&mut project, grandchild).unwrap();
        assert_eq!(delete_bin(&mut project, master), Err(EditError::LastBin));
    }
}
