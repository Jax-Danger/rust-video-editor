//! Nested sequences (compound clips).
//!
//! A nested clip on the parent timeline points at a child [`Sequence`] stored in
//! the project. Its `source_in` / `source_out` are child-sequence frames.
//! Preview and export composite the child at that frame and paint the result as
//! one layer on the parent.

use crate::edit::{
    link_clips, source_frame_at, timeline_frame_at_source, EditError,
};
use crate::mix::mix_regions;
use crate::model::{
    Clip, ClipId, MediaAsset, NestedBinding, Project, Sequence, SequenceId, TrackId, TrackKind,
};
use crate::multicam::{multicam_audio_spans, AudibleSpan};
use crate::time::{Frame, Timebase};

pub const MAX_NEST_DEPTH: u32 = 8;

/// Child-sequence frame for a nested clip at a parent timeline frame.
pub fn nested_frame_at(clip: &Clip, timeline: Frame, sequence_tb: Timebase) -> Frame {
    source_frame_at(clip, timeline, sequence_tb)
}

/// Audio regions for a nested clip. `None` when the clip is not nested.
pub fn nested_audio_spans(
    clip: &Clip,
    child: &Sequence,
    groups: &[crate::model::MulticamGroup],
    media: &[MediaAsset],
    parent_tb: Timebase,
) -> Option<Vec<AudibleSpan>> {
    if clip.nested.is_none() {
        return None;
    }
    let child_start = clip.source_in.0;
    let child_end = clip.source_out.0;
    if child_end <= child_start {
        return Some(Vec::new());
    }
    let mut spans = Vec::new();
    for region in mix_regions(child, child_start, child_end, false) {
        let child_clip = child
            .tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .find(|item| item.id.0 == region.clip_id)?;
        if let Some(mc_spans) =
            multicam_audio_spans(child_clip, groups, media, child.timebase)
        {
            for span in mc_spans {
                let parent_in =
                    timeline_frame_at_source(clip, Frame(child_start + (span.timeline_in - child_start)), parent_tb)
                        .0;
                let parent_out =
                    timeline_frame_at_source(clip, Frame(child_start + (span.timeline_out - child_start)), parent_tb)
                        .0;
                if parent_out > parent_in {
                    spans.push(AudibleSpan {
                        timeline_in: parent_in,
                        timeline_out: parent_out,
                        media_id: span.media_id,
                        source_at_in: span.source_at_in,
                        seconds_per_frame: span.seconds_per_frame,
                    });
                }
            }
            continue;
        }
        let Some(media_id) = child_clip.media_id else {
            continue;
        };
        let Some(asset) = media.iter().find(|item| item.id == media_id) else {
            continue;
        };
        if !asset.has_audio {
            continue;
        }
        let parent_in =
            timeline_frame_at_source(clip, Frame(region.timeline_in), parent_tb).0;
        let parent_out =
            timeline_frame_at_source(clip, Frame(region.timeline_out), parent_tb).0;
        if parent_out <= parent_in {
            continue;
        }
        let src_in = source_frame_at(child_clip, Frame(region.timeline_in), child.timebase);
        let src_next =
            source_frame_at(child_clip, Frame(region.timeline_in + 1), child.timebase);
        let mut seconds_per_frame =
            (src_next.0 - src_in.0) as f64 * child_clip.media_timebase.frame_duration_secs();
        if seconds_per_frame <= 0.0 {
            seconds_per_frame = child.timebase.frame_duration_secs();
        }
        spans.push(AudibleSpan {
            timeline_in: parent_in,
            timeline_out: parent_out,
            media_id,
            source_at_in: src_in.to_seconds(child_clip.media_timebase).max(0.0),
            seconds_per_frame,
        });
    }
    Some(spans)
}

/// True when placing `child` inside `parent` would create a cycle.
pub fn would_cycle(project: &Project, parent: SequenceId, child: SequenceId) -> bool {
    if parent == child {
        return true;
    }
    let mut stack = vec![parent];
    let mut seen = std::collections::HashSet::new();
    while let Some(seq_id) = stack.pop() {
        if seq_id == child {
            return true;
        }
        if !seen.insert(seq_id) {
            continue;
        }
        let Some(sequence) = project.sequence(seq_id) else {
            continue;
        };
        for clip in sequence
            .tracks
            .iter()
            .flat_map(|track| track.clips.iter())
        {
            if let Some(binding) = &clip.nested {
                stack.push(binding.sequence);
            }
        }
    }
    false
}

/// Nest the selected clips into a new child sequence and replace them with one
/// nested clip per affected track (video and audio linked when both exist).
pub fn create_nested_sequence(
    project: &mut Project,
    parent_seq_id: SequenceId,
    clip_ids: &[ClipId],
) -> Result<ClipId, EditError> {
    if clip_ids.is_empty() {
        return Err(EditError::NothingSelected);
    }
    let expanded = {
        let sequence = project
            .sequence(parent_seq_id)
            .ok_or(EditError::SequenceNotFound)?;
        crate::edit::expand_linked(sequence, clip_ids)
    };
    let (placements, min_in, max_out, parent_tb, width, height) = {
        let sequence = project
            .sequence(parent_seq_id)
            .ok_or(EditError::SequenceNotFound)?;
        let mut placements = Vec::new();
        let mut min_in = i64::MAX;
        let mut max_out = i64::MIN;
        for id in &expanded {
            let (ti, ci) = sequence.locate_clip(*id).ok_or(EditError::ClipNotFound)?;
            let track = &sequence.tracks[ti];
            if track.locked {
                return Err(EditError::TrackLocked);
            }
            let clip = &track.clips[ci];
            if clip.nested.is_some() {
                return Err(EditError::AlreadyNested);
            }
            if clip.multicam.is_some() {
                return Err(EditError::NotNestable);
            }
            min_in = min_in.min(clip.timeline_in.0);
            max_out = max_out.max(clip.timeline_out.0);
            placements.push((ti, ci, clip.clone()));
        }
        if min_in == i64::MAX || max_out <= min_in {
            return Err(EditError::InvalidDuration);
        }
        (
            placements,
            min_in,
            max_out,
            sequence.timebase,
            sequence.width,
            sequence.height,
        )
    };
    let span = max_out - min_in;
    let count = project
        .sequences
        .iter()
        .filter(|seq| seq.name.starts_with("Nested"))
        .count()
        + 1;
    let child_name = if count == 1 {
        "Nested".to_string()
    } else {
        format!("Nested {count}")
    };
    let child_id = SequenceId(project.alloc());
    let nest_label = child_name.clone();
    let mut child = Sequence::new(child_id, child_name, width, height, parent_tb);
    let mut track_map: Vec<(usize, TrackId)> = Vec::new();
    for (ti, _, _) in &placements {
        if track_map.iter().any(|(idx, _)| *idx == *ti) {
            continue;
        }
        let (kind, name) = {
            let parent_track = &project.sequence(parent_seq_id).unwrap().tracks[*ti];
            (parent_track.kind, parent_track.name.clone())
        };
        let track_id = TrackId(project.alloc());
        child.add_track(track_id, kind, &name);
        track_map.push((*ti, track_id));
    }
    for (parent_ti, _, clip) in &placements {
        let child_track_id = track_map
            .iter()
            .find(|(idx, _)| *idx == *parent_ti)
            .map(|(_, id)| *id)
            .ok_or(EditError::TrackNotFound)?;
        let child_track = child
            .tracks
            .iter_mut()
            .find(|track| track.id == child_track_id)
            .ok_or(EditError::TrackNotFound)?;
        let mut nested_clip = clip.clone();
        nested_clip.id = ClipId(project.alloc());
        nested_clip.timeline_in = Frame(clip.timeline_in.0 - min_in);
        nested_clip.timeline_out = Frame(clip.timeline_out.0 - min_in);
        nested_clip.linked.clear();
        child_track.clips.push(nested_clip);
    }
    for track in &mut child.tracks {
        track.reindex();
    }
    let child_extent = child.end_frame().0.max(span);
    project.sequences.push(child);
    if would_cycle(project, parent_seq_id, child_id) {
        project.sequences.pop();
        return Err(EditError::NestedCycle);
    }

    let track_kinds: Vec<(usize, TrackKind)> = {
        let parent = project
            .sequence(parent_seq_id)
            .ok_or(EditError::SequenceNotFound)?;
        track_map
            .iter()
            .map(|(parent_ti, _)| (*parent_ti, parent.tracks[*parent_ti].kind))
            .collect()
    };
    let clip_ids: Vec<ClipId> = track_kinds
        .iter()
        .map(|_| ClipId(project.alloc()))
        .collect();
    let parent = project
        .sequence_mut(parent_seq_id)
        .ok_or(EditError::SequenceNotFound)?;
    let remove_ids: Vec<ClipId> = placements.iter().map(|(_, _, clip)| clip.id).collect();
    for track in &mut parent.tracks {
        track.clips.retain(|clip| !remove_ids.contains(&clip.id));
        track.reindex();
    }
    cleanup_transitions(parent);

    let binding = NestedBinding { sequence: child_id };
    let mut primary = None;
    let mut video_clip = None;
    let mut audio_clip = None;
    let mut nested_clips = Vec::new();
    for ((parent_ti, kind), clip_id) in track_kinds.iter().zip(clip_ids.iter()) {
        let mut nested = nested_parent_clip(
            *clip_id,
            &nest_label,
            Frame(min_in),
            span,
            parent_tb,
            *kind,
            binding.clone(),
        );
        nested = nested.with_handles(0, child_extent.saturating_sub(span));
        nested_clips.push((*parent_ti, nested));
        match kind {
            TrackKind::Video => {
                video_clip = nested_clips.last().map(|(_, clip)| clip.clone());
                primary = Some(*clip_id);
            }
            TrackKind::Audio => {
                audio_clip = nested_clips.last().map(|(_, clip)| clip.clone());
            }
            TrackKind::Caption => {}
        }
    }
    for (parent_ti, nested) in nested_clips {
        parent.tracks[parent_ti].clips.push(nested);
        parent.tracks[parent_ti].reindex();
    }
    if let (Some(mut video), Some(mut audio)) = (video_clip, audio_clip) {
        link_clips(&mut video, &mut audio);
        for track in &mut parent.tracks {
            if let Some(ci) = track.clips.iter().position(|clip| clip.id == video.id) {
                track.clips[ci] = video.clone();
            }
            if let Some(ci) = track.clips.iter().position(|clip| clip.id == audio.id) {
                track.clips[ci] = audio.clone();
            }
        }
    }
    primary.ok_or(EditError::ClipNotFound)
}

fn nested_parent_clip(
    id: ClipId,
    name: &str,
    at: Frame,
    span: i64,
    tb: Timebase,
    kind: TrackKind,
    binding: NestedBinding,
) -> Clip {
    Clip {
        id,
        media_id: None,
        name: name.to_string(),
        timeline_in: at,
        timeline_out: Frame(at.0 + span),
        source_in: Frame::ZERO,
        source_out: Frame(span),
        source_min: Frame::ZERO,
        source_max: Frame(span),
        media_timebase: tb,
        linked: Vec::new(),
        enabled: true,
        effects: Vec::new(),
        label: match kind {
            TrackKind::Video => crate::model::LabelColor::Teal,
            TrackKind::Audio => crate::model::LabelColor::Green,
            TrackKind::Caption => crate::model::LabelColor::Neutral,
        },
        volume: crate::effects::AnimatedF32::constant(1.0),
        title: None,
        adjustment: false,
        speed: crate::model::ClipSpeed::normal(),
        multicam: None,
        nested: Some(binding),
        track_matte: None,
    }
}

fn cleanup_transitions(sequence: &mut Sequence) {
    for track in &mut sequence.tracks {
        let ids: std::collections::HashSet<ClipId> = track.clips.iter().map(|clip| clip.id).collect();
        track
            .transitions
            .retain(|transition| ids.contains(&transition.left_clip) && ids.contains(&transition.right_clip));
    }
}

/// Child sequence id when `clip` is nested.
pub fn nested_sequence_id(clip: &Clip) -> Option<SequenceId> {
    clip.nested.as_ref().map(|binding| binding.sequence)
}

/// Resolve the child sequence for a nested clip.
pub fn nested_sequence<'a>(project: &'a Project, clip: &Clip) -> Option<&'a Sequence> {
    nested_sequence_id(clip).and_then(|id| project.sequence(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MediaAsset, MediaId, TrackId};

    fn project_with_clips() -> (Project, SequenceId, ClipId, ClipId) {
        let mut project = Project::new("nest");
        let seq_id = SequenceId(project.alloc());
        let mut sequence = Sequence::new(seq_id, "Main", 1920, 1080, Timebase::fps_24());
        let v = TrackId(project.alloc());
        let a = TrackId(project.alloc());
        sequence.add_track(v, TrackKind::Video, "V1");
        sequence.add_track(a, TrackKind::Audio, "A1");
        let video = ClipId(project.alloc());
        let audio = ClipId(project.alloc());
        let media_id = MediaId(project.alloc());
        project.media.push(MediaAsset {
            id: media_id,
            bin_id: crate::model::BinId(1),
            name: "clip.mp4".into(),
            path: "clip.mp4".into(),
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
        });
        let mut vclip = Clip::basic(video.0, 24, 72);
        vclip.media_id = Some(media_id);
        vclip.media_timebase = Timebase::fps_24();
        vclip.source_max = Frame(240);
        let mut aclip = Clip::basic(audio.0, 24, 72);
        aclip.media_id = Some(media_id);
        aclip.media_timebase = Timebase::fps_24();
        aclip.source_max = Frame(240);
        link_clips(&mut vclip, &mut aclip);
        sequence.tracks[0].clips.push(vclip);
        sequence.tracks[1].clips.push(aclip);
        project.sequences.push(sequence);
        project.active_sequence = Some(seq_id);
        (project, seq_id, video, audio)
    }

    #[test]
    fn create_nested_sequence_replaces_selection() {
        let (mut project, seq, video, audio) = project_with_clips();
        let nested = create_nested_sequence(&mut project, seq, &[video]).unwrap();
        let parent = project.sequence(seq).unwrap();
        assert_eq!(parent.tracks[0].clips.len(), 1);
        assert!(parent.tracks[0].clips[0].is_nested());
        assert_eq!(parent.tracks[0].clips[0].timeline_in, Frame(24));
        assert_eq!(parent.tracks[0].clips[0].timeline_out, Frame(72));
        assert_eq!(parent.tracks[1].clips.len(), 1);
        assert!(parent.tracks[1].clips[0].linked.contains(&nested));
        assert_eq!(project.sequences.len(), 2);
        let child_id = parent.tracks[0].clips[0].nested.as_ref().unwrap().sequence;
        let child = project.sequence(child_id).unwrap();
        assert_eq!(child.tracks[0].clips.len(), 1);
        assert_eq!(child.tracks[0].clips[0].timeline_in, Frame::ZERO);
    }

    #[test]
    fn nested_frame_maps_parent_to_child() {
        let (mut project, seq, video, _) = project_with_clips();
        create_nested_sequence(&mut project, seq, &[video]).unwrap();
        let parent = project.sequence(seq).unwrap();
        let clip = &parent.tracks[0].clips[0];
        assert_eq!(nested_frame_at(clip, Frame(24), parent.timebase), Frame(0));
        assert_eq!(nested_frame_at(clip, Frame(48), parent.timebase), Frame(24));
    }

    #[test]
    fn json_round_trip_keeps_nested_binding() {
        let (mut project, seq, video, _) = project_with_clips();
        create_nested_sequence(&mut project, seq, &[video]).unwrap();
        let json = project.to_json_pretty().unwrap();
        assert!(json.contains("\"nested\""));
        let loaded = Project::from_json(&json).unwrap();
        assert_eq!(loaded.sequences.len(), 2);
        let parent = loaded.sequence(seq).unwrap();
        assert!(parent.tracks[0].clips[0].is_nested());
    }

    #[test]
    fn cycle_detection_blocks_self_nest() {
        let (project, seq, _, _) = project_with_clips();
        assert!(would_cycle(&project, seq, seq));
    }

    #[test]
    fn rejects_multicam_for_nesting() {
        let (mut project, seq, video, _) = project_with_clips();
        project.sequence_mut(seq).unwrap().tracks[0].clips[0].multicam =
            Some(crate::model::MulticamBinding {
                group: crate::model::MulticamId(9),
                cuts: Vec::new(),
            });
        assert!(matches!(
            create_nested_sequence(&mut project, seq, &[video]),
            Err(EditError::NotNestable)
        ));
    }
}
