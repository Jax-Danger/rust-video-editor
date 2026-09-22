//! Multicam groups, sync, and angle cuts.
//!
//! A group is a set of video angles (each with optional dedicated audio) that
//! share one sync origin. Group time 0 is the clap. An angle's `sync_offset`
//! is the source frame on that file which lands on group time 0.
//!
//! A timeline clip stores a [`MulticamBinding`]: which group, and angle cuts in
//! group time. Moving the clip does not move the cuts. Preview and export ask
//! [`picture_at`] / [`multicam_audio_spans`] for the active angle and then use
//! the same composite and mix path as any other clip.

use crate::edit::{
    link_clips, overwrite_clips, razor_clip, source_frame_at, timeline_frame_at_source, EditError,
};
use crate::model::{
    AngleCut, Clip, ClipId, MediaAsset, MediaId, MulticamAngle, MulticamBinding, MulticamGroup,
    MulticamId, Project, Sequence, SequenceId, TrackKind,
};
use crate::time::{convert_frames, Frame, Timebase};

/// Picture the compositor should decode for this clip at `timeline`.
#[derive(Clone, Debug, PartialEq)]
pub struct AnglePicture {
    pub angle: u32,
    pub name: String,
    pub media_id: MediaId,
    pub source_frame: i64,
    pub timebase: Timebase,
}

/// One audio region after an angle cut. Times are sequence frames.
/// `source_at_in` is seconds into the angle's audio at `timeline_in`.
#[derive(Clone, Debug, PartialEq)]
pub struct AudibleSpan {
    pub timeline_in: i64,
    pub timeline_out: i64,
    pub media_id: MediaId,
    pub source_at_in: f64,
    pub seconds_per_frame: f64,
}

/// Last cut at or before `group_time`. No cut yet means angle 0.
pub fn active_angle(cuts: &[AngleCut], group_time: i64) -> u32 {
    let mut best_at = i64::MIN;
    let mut angle = 0u32;
    let mut any = false;
    for cut in cuts {
        if cut.at.0 <= group_time && (!any || cut.at.0 >= best_at) {
            best_at = cut.at.0;
            angle = cut.angle;
            any = true;
        }
    }
    angle
}

/// Source frame on `angle` at `group_frame` (group timebase).
pub fn angle_source_frame(
    angle: &MulticamAngle,
    group_frame: i64,
    group_tb: Timebase,
    media_tb: Timebase,
) -> i64 {
    let mapped = if group_frame == 0 {
        0
    } else {
        convert_frames(group_frame, group_tb, media_tb)
    };
    angle.sync_offset.0.saturating_add(mapped)
}

/// Audio asset for this angle: dedicated audio, otherwise the video file when
/// that file has sound.
pub fn angle_audio_media(angle: &MulticamAngle, media: &[MediaAsset]) -> Option<MediaId> {
    if let Some(id) = angle.audio {
        if media.iter().any(|item| item.id == id && item.has_audio) {
            return Some(id);
        }
    }
    if media
        .iter()
        .any(|item| item.id == angle.video && item.has_audio)
    {
        return Some(angle.video);
    }
    None
}

/// Active angle's picture at `timeline`, or `None` when the clip is not multicam.
pub fn picture_at(
    clip: &Clip,
    groups: &[MulticamGroup],
    media: &[MediaAsset],
    timeline: Frame,
    sequence_tb: Timebase,
) -> Option<AnglePicture> {
    let binding = clip.multicam.as_ref()?;
    let group = groups.iter().find(|item| item.id == binding.group)?;
    if group.angles.is_empty() {
        return None;
    }
    let group_frame = source_frame_at(clip, timeline, sequence_tb).0;
    let mut angle_index = active_angle(&binding.cuts, group_frame);
    if angle_index as usize >= group.angles.len() {
        angle_index = (group.angles.len() - 1) as u32;
    }
    let angle = &group.angles[angle_index as usize];
    let asset = media.iter().find(|item| item.id == angle.video)?;
    let source_frame =
        angle_source_frame(angle, group_frame, group.timebase, asset.timebase).max(0);
    Some(AnglePicture {
        angle: angle_index,
        name: angle.name.clone(),
        media_id: angle.video,
        source_frame,
        timebase: asset.timebase,
    })
}

/// Angle name active at the clip's source in-point.
pub fn opening_angle_name<'a>(clip: &Clip, groups: &'a [MulticamGroup]) -> Option<&'a str> {
    let binding = clip.multicam.as_ref()?;
    let group = groups.iter().find(|item| item.id == binding.group)?;
    let index = active_angle(&binding.cuts, clip.source_in.0) as usize;
    group.angles.get(index).map(|angle| angle.name.as_str())
}

/// Timeline frames of angle cuts that fall strictly inside the clip.
pub fn angle_marks(clip: &Clip, sequence_tb: Timebase) -> Vec<(i64, u32)> {
    let Some(binding) = clip.multicam.as_ref() else {
        return Vec::new();
    };
    let mut marks = Vec::new();
    for cut in &binding.cuts {
        if cut.at.0 <= clip.source_in.0 || cut.at.0 >= clip.source_out.0 {
            continue;
        }
        let frame = timeline_frame_at_source(clip, cut.at, sequence_tb).0;
        if frame > clip.timeline_in.0 && frame < clip.timeline_out.0 {
            marks.push((frame, cut.angle));
        }
    }
    marks
}

/// Audio pieces for a multicam clip.
///
/// `None` means the clip is not multicam (the caller keeps its own mapping).
/// `Some` is the angle breakdown, and it is empty when no angle has audio.
pub fn multicam_audio_spans(
    clip: &Clip,
    groups: &[MulticamGroup],
    media: &[MediaAsset],
    sequence_tb: Timebase,
) -> Option<Vec<AudibleSpan>> {
    let binding = clip.multicam.as_ref()?;
    let group = groups.iter().find(|item| item.id == binding.group)?;
    let start_g = source_frame_at(clip, clip.timeline_in, sequence_tb).0;
    let end_g = source_frame_at(clip, clip.timeline_out, sequence_tb).0;
    if end_g <= start_g {
        return Some(Vec::new());
    }
    let mut bounds = vec![start_g];
    for cut in &binding.cuts {
        if cut.at.0 > start_g && cut.at.0 < end_g {
            bounds.push(cut.at.0);
        }
    }
    bounds.push(end_g);
    bounds.sort_unstable();
    bounds.dedup();
    let mut spans = Vec::new();
    for pair in bounds.windows(2) {
        let g0 = pair[0];
        let g1 = pair[1];
        let angle_index = active_angle(&binding.cuts, g0);
        let Some(angle) = group.angles.get(angle_index as usize) else {
            continue;
        };
        let Some(media_id) = angle_audio_media(angle, media) else {
            continue;
        };
        let Some(asset) = media.iter().find(|item| item.id == media_id) else {
            continue;
        };
        let t0 = timeline_frame_at_source(clip, Frame(g0), sequence_tb).0;
        let t1 = timeline_frame_at_source(clip, Frame(g1), sequence_tb).0;
        if t1 <= t0 {
            continue;
        }
        let src0 = angle_source_frame(angle, g0, group.timebase, asset.timebase).max(0);
        let g_next = source_frame_at(clip, Frame(t0 + 1), sequence_tb).0;
        let src_next = angle_source_frame(angle, g_next, group.timebase, asset.timebase).max(0);
        let mut seconds_per_frame = (src_next - src0) as f64 * asset.timebase.frame_duration_secs();
        if seconds_per_frame <= 0.0 {
            seconds_per_frame = sequence_tb.frame_duration_secs();
        }
        spans.push(AudibleSpan {
            timeline_in: t0,
            timeline_out: t1,
            media_id,
            source_at_in: Frame(src0).to_seconds(asset.timebase),
            seconds_per_frame,
        });
    }
    Some(spans)
}

/// Multicam clip the editor should switch: the selected one under the playhead,
/// otherwise the topmost visible video multicam.
pub fn multicam_target(sequence: &Sequence, selected: &[ClipId], playhead: i64) -> Option<ClipId> {
    let frame = Frame(playhead);
    let selected_hits: Vec<ClipId> = selected
        .iter()
        .copied()
        .filter(|id| {
            sequence
                .clip(*id)
                .is_some_and(|clip| clip.multicam.is_some() && clip.covers(frame))
        })
        .collect();
    if let Some(id) = selected_hits.iter().copied().find(|id| {
        sequence
            .locate_clip(*id)
            .is_some_and(|(ti, _)| sequence.tracks[ti].kind == TrackKind::Video)
    }) {
        return Some(id);
    }
    if let Some(id) = selected_hits.first() {
        return Some(*id);
    }
    let mut found = None;
    for track in sequence
        .tracks
        .iter()
        .filter(|track| video_track_visible(track, &sequence.tracks))
    {
        if let Some(clip) = track
            .clips
            .iter()
            .find(|clip| clip.enabled && clip.covers(frame) && clip.multicam.is_some())
        {
            found = Some(clip.id);
        }
    }
    found
}

fn video_track_visible(track: &crate::model::Track, tracks: &[crate::model::Track]) -> bool {
    if track.kind != TrackKind::Video || track.muted {
        return false;
    }
    let any_solo = tracks
        .iter()
        .any(|item| item.kind == TrackKind::Video && item.solo);
    !any_solo || track.solo
}

/// Build a group from pool items and overwrite it onto the timeline at `at`.
///
/// Video items become angles in selection order. Audio-only items attach, in
/// order, as dedicated angle audio. The clip length is the overlap of the
/// angles; a longer angle is left as a tail handle. Returns the video clip id.
pub fn create_multicam(
    project: &mut Project,
    sequence_id: SequenceId,
    media_ids: &[MediaId],
    at: Frame,
) -> Result<ClipId, EditError> {
    if at.0 < 0 {
        return Err(EditError::OutOfRange);
    }
    let mut seen = std::collections::HashSet::new();
    let mut ordered = Vec::new();
    for id in media_ids {
        if seen.insert(*id) {
            ordered.push(*id);
        }
    }
    let mut videos = Vec::new();
    let mut audios = Vec::new();
    for id in ordered {
        let asset = project.media(id).cloned().ok_or(EditError::MediaNotFound)?;
        if asset.has_video {
            videos.push(asset);
        } else if asset.has_audio {
            audios.push(asset);
        }
    }
    if videos.len() < 2 {
        return Err(EditError::NotEnoughAngles);
    }
    let sequence = project
        .sequence(sequence_id)
        .ok_or(EditError::SequenceNotFound)?;
    let group_tb = sequence.timebase;
    let video_track = sequence
        .tracks
        .iter()
        .find(|track| track.kind == TrackKind::Video && !track.locked)
        .map(|track| track.id)
        .ok_or(EditError::WrongTrackKind)?;
    let audio_track = sequence
        .tracks
        .iter()
        .find(|track| track.kind == TrackKind::Audio && !track.locked)
        .map(|track| track.id);

    let mut angles = Vec::new();
    let mut spans = Vec::new();
    for (index, asset) in videos.iter().enumerate() {
        let available = group_span(asset, 0, group_tb);
        if available < 1 {
            return Err(EditError::InvalidDuration);
        }
        spans.push(available);
        let audio = audios.get(index).map(|item| item.id);
        angles.push(MulticamAngle {
            name: angle_label(&asset.name, index),
            video: asset.id,
            audio,
            sync_offset: Frame::ZERO,
        });
    }
    let overlap = spans.iter().copied().min().unwrap_or(1).max(1);
    let extent = spans.iter().copied().max().unwrap_or(overlap).max(overlap);
    let count = project.multicam_groups.len() + 1;
    let name = if count == 1 {
        "Multicam".to_string()
    } else {
        format!("Multicam {count}")
    };
    let group_id = MulticamId(project.alloc());
    project.multicam_groups.push(MulticamGroup {
        id: group_id,
        name: name.clone(),
        timebase: group_tb,
        angles,
    });
    let binding = MulticamBinding {
        group: group_id,
        cuts: vec![AngleCut {
            at: Frame::ZERO,
            angle: 0,
        }],
    };
    let video_id = ClipId(project.alloc());
    let opening = &videos[0];
    let mut video = multicam_clip(
        video_id,
        &name,
        at,
        overlap,
        extent,
        group_tb,
        Some(opening.id),
        binding.clone(),
        true,
    );
    let mut placed = vec![(video_track, video.clone())];
    let group = project
        .multicam_group(group_id)
        .ok_or(EditError::GroupNotFound)?;
    let audio_media = group
        .angles
        .iter()
        .find_map(|angle| angle_audio_media(angle, &project.media));
    if let (Some(track), Some(media_id)) = (audio_track, audio_media) {
        let audio_id = ClipId(project.alloc());
        let mut audio = multicam_clip(
            audio_id,
            &name,
            at,
            overlap,
            extent,
            group_tb,
            Some(media_id),
            binding,
            false,
        );
        link_clips(&mut video, &mut audio);
        placed[0].1 = video;
        placed.push((track, audio));
    }
    overwrite_clips(project, sequence_id, placed)?;
    Ok(video_id)
}

fn group_span(asset: &MediaAsset, sync: i64, group_tb: Timebase) -> i64 {
    let available = asset.duration.0.saturating_sub(sync.max(0));
    if available <= 0 {
        return 0;
    }
    convert_frames(available, asset.timebase, group_tb).max(1)
}

fn angle_label(name: &str, index: usize) -> String {
    let stem = std::path::Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(name)
        .trim();
    if stem.is_empty() {
        format!("Angle {}", index + 1)
    } else {
        stem.to_string()
    }
}

fn multicam_clip(
    id: ClipId,
    name: &str,
    at: Frame,
    overlap: i64,
    extent: i64,
    group_tb: Timebase,
    media_id: Option<MediaId>,
    binding: MulticamBinding,
    video: bool,
) -> Clip {
    Clip {
        id,
        media_id,
        name: name.to_string(),
        timeline_in: at,
        timeline_out: Frame(at.0 + overlap),
        source_in: Frame::ZERO,
        source_out: Frame(overlap),
        source_min: Frame::ZERO,
        source_max: Frame(extent),
        media_timebase: group_tb,
        linked: Vec::new(),
        enabled: true,
        effects: Vec::new(),
        label: if video {
            crate::model::LabelColor::Violet
        } else {
            crate::model::LabelColor::Green
        },
        volume: crate::effects::AnimatedF32::constant(1.0),
        title: None,
        adjustment: false,
        speed: crate::model::ClipSpeed::normal(),
        multicam: Some(binding),
    }
}

/// Switch the angle at `at`.
///
/// Inside the clip this razors first (linked partners split too) and the
/// right-hand piece takes `angle`. On the clip's first frame the whole clip
/// retargets. Returns the clip id that now plays the angle.
pub fn switch_angle(
    project: &mut Project,
    sequence_id: SequenceId,
    clip_id: ClipId,
    at: Frame,
    angle: u32,
) -> Result<ClipId, EditError> {
    let (group_id, split, mut partners) = {
        let sequence = project
            .sequence(sequence_id)
            .ok_or(EditError::SequenceNotFound)?;
        let clip = sequence.clip(clip_id).ok_or(EditError::ClipNotFound)?;
        let binding = clip.multicam.clone().ok_or(EditError::NotMulticam)?;
        if !clip.covers(at) {
            return Err(EditError::NotInsideClip);
        }
        let group = project
            .multicam_group(binding.group)
            .ok_or(EditError::GroupNotFound)?;
        if angle as usize >= group.angles.len() {
            return Err(EditError::AngleOutOfRange);
        }
        let mut partners = vec![clip_id];
        for linked in &clip.linked {
            if sequence
                .clip(*linked)
                .is_some_and(|partner| partner.covers(at))
            {
                partners.push(*linked);
            }
        }
        (binding.group, clip.contains_frame(at), partners)
    };
    if split {
        razor_clip(project, sequence_id, clip_id, at)?;
    }
    let angle_def = project
        .multicam_group(group_id)
        .ok_or(EditError::GroupNotFound)?
        .angles
        .get(angle as usize)
        .cloned()
        .ok_or(EditError::AngleOutOfRange)?;
    let audio_media = angle_audio_media(&angle_def, &project.media);
    let sequence = project
        .sequence_mut(sequence_id)
        .ok_or(EditError::SequenceNotFound)?;
    let timebase = sequence.timebase;
    if split {
        for id in &partners {
            if let Some((ti, ci)) = sequence.locate_clip(*id) {
                let clip = &mut sequence.tracks[ti].clips[ci];
                if let Some(binding) = clip.multicam.as_mut() {
                    if binding.group == group_id {
                        let end = clip.source_out.0;
                        binding.cuts.retain(|cut| cut.at.0 < end);
                    }
                }
            }
        }
    }
    let targets: Vec<(usize, usize)> = if split {
        let mut found = Vec::new();
        for (ti, track) in sequence.tracks.iter().enumerate() {
            for (ci, clip) in track.clips.iter().enumerate() {
                let same = clip
                    .multicam
                    .as_ref()
                    .is_some_and(|binding| binding.group == group_id);
                if same && clip.timeline_in == at && !partners.contains(&clip.id) {
                    found.push((ti, ci));
                }
            }
        }
        found
    } else {
        partners.retain(|id| {
            sequence.clip(*id).is_some_and(|clip| {
                clip.covers(at)
                    && clip
                        .multicam
                        .as_ref()
                        .is_some_and(|binding| binding.group == group_id)
            })
        });
        if !partners.contains(&clip_id) {
            partners.insert(0, clip_id);
        }
        partners
            .iter()
            .filter_map(|id| sequence.locate_clip(*id))
            .collect()
    };
    if targets.is_empty() {
        return Err(EditError::ClipNotFound);
    }
    let mut primary = None;
    for (ti, ci) in targets {
        let kind = sequence.tracks[ti].kind;
        let clip = &mut sequence.tracks[ti].clips[ci];
        let group_time = source_frame_at(clip, at, timebase).0;
        if let Some(binding) = clip.multicam.as_mut() {
            let source_in = clip.source_in.0;
            binding
                .cuts
                .retain(|cut| cut.at.0 < group_time && cut.at.0 >= source_in);
            binding.cuts.push(AngleCut {
                at: Frame(group_time),
                angle,
            });
            binding.cuts.sort_by_key(|cut| cut.at.0);
        }
        clip.media_id = match kind {
            TrackKind::Video => Some(angle_def.video),
            TrackKind::Audio => audio_media,
            TrackKind::Caption => clip.media_id,
        };
        if kind == TrackKind::Video && primary.is_none() {
            primary = Some(clip.id);
        }
        if primary.is_none() {
            primary = Some(clip.id);
        }
    }
    primary.ok_or(EditError::ClipNotFound)
}

/// Slip one angle against the group. `offset` is a source frame on that angle.
pub fn set_angle_sync(
    project: &mut Project,
    group_id: MulticamId,
    angle: u32,
    offset: i64,
) -> Result<(), EditError> {
    if offset < 0 {
        return Err(EditError::OutOfRange);
    }
    let group = project
        .multicam_group_mut(group_id)
        .ok_or(EditError::GroupNotFound)?;
    let angle = group
        .angles
        .get_mut(angle as usize)
        .ok_or(EditError::AngleOutOfRange)?;
    angle.sync_offset = Frame(offset);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SequenceId, TrackId};
    use crate::time::Timebase;

    fn asset(
        id: u64,
        name: &str,
        frames: i64,
        tb: Timebase,
        video: bool,
        audio: bool,
    ) -> MediaAsset {
        MediaAsset {
            id: MediaId(id),
            bin_id: crate::model::BinId(1),
            name: name.into(),
            path: name.into(),
            duration: Frame(frames),
            timebase: tb,
            width: video.then_some(1920),
            height: video.then_some(1080),
            video_codec: video.then(|| "h264".into()),
            audio_codec: audio.then(|| "aac".into()),
            audio_channels: audio.then_some(2),
            sample_rate: audio.then_some(48_000),
            has_video: video,
            has_audio: audio,
            offline: false,
            proxy_path: None,
        }
    }

    fn project_with_two_angles() -> (Project, SequenceId) {
        let mut project = Project::new("mc");
        let bin_id = crate::model::BinId(project.alloc());
        project.bins.push(crate::model::Bin {
            id: bin_id,
            name: "Master".into(),
            parent: None,
        });
        let tb = Timebase::fps_24();
        let seq = SequenceId(project.alloc());
        let mut sequence = Sequence::new(seq, "Timeline", 1920, 1080, tb);
        sequence.add_track(TrackId(project.alloc()), TrackKind::Video, "V1");
        sequence.add_track(TrackId(project.alloc()), TrackKind::Audio, "A1");
        project.sequences.push(sequence);
        project.active_sequence = Some(seq);
        let wide = MediaId(project.alloc());
        let tight = MediaId(project.alloc());
        project
            .media
            .push(asset(wide.0, "wide.mp4", 200, tb, true, true));
        project
            .media
            .push(asset(tight.0, "tight.mp4", 180, tb, true, true));
        (project, seq)
    }

    #[test]
    fn sync_offset_shifts_the_angle_source() {
        let (mut project, seq) = project_with_two_angles();
        let ids: Vec<MediaId> = project.media.iter().map(|item| item.id).collect();
        let clip_id = create_multicam(&mut project, seq, &ids, Frame(0)).unwrap();
        let group_id = project.multicam_groups[0].id;
        set_angle_sync(&mut project, group_id, 1, 12).unwrap();
        let sequence = project.sequence(seq).unwrap();
        let clip = sequence.clip(clip_id).unwrap();
        let groups = &project.multicam_groups;
        let media = &project.media;
        let wide = picture_at(clip, groups, media, Frame(0), sequence.timebase).unwrap();
        assert_eq!(wide.angle, 0);
        assert_eq!(wide.source_frame, 0);
        assert_eq!(wide.name, "wide");
        switch_angle(&mut project, seq, clip_id, Frame(0), 1).unwrap();
        let sequence = project.sequence(seq).unwrap();
        let clip = sequence
            .tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .find(|clip| {
                clip.multicam.is_some()
                    && track_is_video(sequence, clip.id)
                    && clip.covers(Frame(0))
            })
            .unwrap();
        let tight = picture_at(
            clip,
            &project.multicam_groups,
            &project.media,
            Frame(8),
            sequence.timebase,
        )
        .unwrap();
        assert_eq!(tight.angle, 1);
        assert_eq!(tight.source_frame, 20, "sync 12 plus group frame 8");
        assert_eq!(tight.name, "tight");
        let wide_still = {
            let group = &project.multicam_groups[0];
            angle_source_frame(&group.angles[0], 8, group.timebase, Timebase::fps_24())
        };
        assert_eq!(wide_still, 8);
    }

    fn track_is_video(sequence: &Sequence, id: ClipId) -> bool {
        sequence
            .locate_clip(id)
            .is_some_and(|(ti, _)| sequence.tracks[ti].kind == TrackKind::Video)
    }

    #[test]
    fn switch_angle_razors_and_keeps_sync_on_the_right_piece() {
        let (mut project, seq) = project_with_two_angles();
        let ids: Vec<MediaId> = project.media.iter().map(|item| item.id).collect();
        let clip_id = create_multicam(&mut project, seq, &ids, Frame(0)).unwrap();
        let group_id = project.multicam_groups[0].id;
        set_angle_sync(&mut project, group_id, 1, 12).unwrap();
        let right = switch_angle(&mut project, seq, clip_id, Frame(10), 1).unwrap();
        let sequence = project.sequence(seq).unwrap();
        let left = sequence.clip(clip_id).unwrap();
        let right_clip = sequence.clip(right).unwrap();
        assert_eq!(left.timeline_in.0, 0);
        assert_eq!(left.timeline_out.0, 10);
        assert_eq!(right_clip.timeline_in.0, 10);
        assert_eq!(right_clip.timeline_out.0, 180);
        let before = picture_at(
            left,
            &project.multicam_groups,
            &project.media,
            Frame(9),
            sequence.timebase,
        )
        .unwrap();
        assert_eq!(before.angle, 0);
        assert_eq!(before.source_frame, 9);
        let after = picture_at(
            right_clip,
            &project.multicam_groups,
            &project.media,
            Frame(10),
            sequence.timebase,
        )
        .unwrap();
        assert_eq!(after.angle, 1);
        assert_eq!(after.source_frame, 22);
        assert_eq!(after.media_id, project.media[1].id);
        let audio = sequence
            .tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Audio)
            .flat_map(|track| track.clips.iter())
            .find(|clip| clip.timeline_in.0 == 10)
            .unwrap();
        let spans = multicam_audio_spans(
            audio,
            &project.multicam_groups,
            &project.media,
            sequence.timebase,
        )
        .unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].media_id, project.media[1].id);
        assert!((spans[0].source_at_in - 22.0 / 24.0).abs() < 1.0e-6);
        let marks = angle_marks(right_clip, sequence.timebase);
        assert!(
            marks.is_empty(),
            "the cut is the clip in-point, not an interior mark"
        );
    }

    #[test]
    fn interior_cuts_split_audio_without_a_razor() {
        let (mut project, seq) = project_with_two_angles();
        let ids: Vec<MediaId> = project.media.iter().map(|item| item.id).collect();
        create_multicam(&mut project, seq, &ids, Frame(0)).unwrap();
        let (audio, tb) = {
            let sequence = project.sequence_mut(seq).unwrap();
            let audio = sequence
                .tracks
                .iter_mut()
                .filter(|track| track.kind == TrackKind::Audio)
                .flat_map(|track| track.clips.iter_mut())
                .next()
                .unwrap();
            audio.multicam.as_mut().unwrap().cuts.push(AngleCut {
                at: Frame(20),
                angle: 1,
            });
            (audio.clone(), sequence.timebase)
        };
        let spans =
            multicam_audio_spans(&audio, &project.multicam_groups, &project.media, tb).unwrap();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].timeline_in, 0);
        assert_eq!(spans[0].timeline_out, 20);
        assert_eq!(spans[0].media_id, project.media[0].id);
        assert_eq!(spans[1].timeline_in, 20);
        assert_eq!(spans[1].media_id, project.media[1].id);
    }

    #[test]
    fn moving_the_clip_keeps_the_group_time_cut() {
        let tb = Timebase::fps_24();
        let mut clip = Clip::basic(1, 100, 140);
        clip.media_timebase = tb;
        clip.source_in = Frame(0);
        clip.source_out = Frame(40);
        clip.source_max = Frame(40);
        clip.multicam = Some(MulticamBinding {
            group: MulticamId(9),
            cuts: vec![
                AngleCut {
                    at: Frame(0),
                    angle: 0,
                },
                AngleCut {
                    at: Frame(10),
                    angle: 1,
                },
            ],
        });
        let groups = vec![MulticamGroup {
            id: MulticamId(9),
            name: "Multicam".into(),
            timebase: tb,
            angles: vec![
                MulticamAngle {
                    name: "A".into(),
                    video: MediaId(1),
                    audio: None,
                    sync_offset: Frame(0),
                },
                MulticamAngle {
                    name: "B".into(),
                    video: MediaId(2),
                    audio: None,
                    sync_offset: Frame(7),
                },
            ],
        }];
        let media = vec![
            asset(1, "a.mp4", 80, tb, true, false),
            asset(2, "b.mp4", 80, tb, true, false),
        ];
        let early = picture_at(&clip, &groups, &media, Frame(100), tb).unwrap();
        assert_eq!(early.angle, 0);
        assert_eq!(early.source_frame, 0);
        let late = picture_at(&clip, &groups, &media, Frame(110), tb).unwrap();
        assert_eq!(late.angle, 1);
        assert_eq!(late.source_frame, 17);
        assert_eq!(angle_marks(&clip, tb), vec![(110, 1)]);
    }

    #[test]
    fn forty_eight_fps_angle_maps_group_frames() {
        let group_tb = Timebase::fps_24();
        let media_tb = Timebase::new(48, 1);
        let angle = MulticamAngle {
            name: "fast".into(),
            video: MediaId(1),
            audio: None,
            sync_offset: Frame(10),
        };
        assert_eq!(angle_source_frame(&angle, 4, group_tb, media_tb), 18);
    }

    #[test]
    fn json_round_trip_keeps_groups_cuts_and_sync() {
        let (mut project, seq) = project_with_two_angles();
        let ids: Vec<MediaId> = project.media.iter().map(|item| item.id).collect();
        let clip_id = create_multicam(&mut project, seq, &ids, Frame(12)).unwrap();
        let group_id = project.multicam_groups[0].id;
        set_angle_sync(&mut project, group_id, 1, 6).unwrap();
        switch_angle(&mut project, seq, clip_id, Frame(20), 1).unwrap();
        let json = project.to_json_pretty().unwrap();
        assert!(json.contains("multicam_groups"));
        assert!(json.contains("sync_offset"));
        let loaded = Project::from_json(&json).unwrap();
        assert_eq!(loaded.multicam_groups.len(), 1);
        assert_eq!(loaded.multicam_groups[0].angles[1].sync_offset, Frame(6));
        let sequence = loaded.sequence(seq).unwrap();
        let right = sequence
            .tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Video)
            .flat_map(|track| track.clips.iter())
            .find(|clip| clip.timeline_in.0 == 20)
            .unwrap();
        let picture = picture_at(
            right,
            &loaded.multicam_groups,
            &loaded.media,
            Frame(20),
            sequence.timebase,
        )
        .unwrap();
        assert_eq!(picture.angle, 1);
        assert_eq!(picture.source_frame, 6 + 8);
        let plain = Project::from_json(r#"{"format_version":1,"name":"old","next_id":1}"#).unwrap();
        assert!(plain.multicam_groups.is_empty());
    }

    #[test]
    fn opening_switch_does_not_split() {
        let (mut project, seq) = project_with_two_angles();
        let ids: Vec<MediaId> = project.media.iter().map(|item| item.id).collect();
        let clip_id = create_multicam(&mut project, seq, &ids, Frame(4)).unwrap();
        let returned = switch_angle(&mut project, seq, clip_id, Frame(4), 1).unwrap();
        assert_eq!(returned, clip_id);
        let sequence = project.sequence(seq).unwrap();
        let videos: Vec<_> = sequence
            .tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Video)
            .flat_map(|track| track.clips.iter())
            .collect();
        assert_eq!(videos.len(), 1);
        assert_eq!(videos[0].media_id, Some(project.media[1].id));
    }

    #[test]
    fn one_video_is_rejected_and_audio_only_attaches() {
        let (mut project, seq) = project_with_two_angles();
        let only = project.media[0].id;
        assert!(matches!(
            create_multicam(&mut project, seq, &[only], Frame(0)),
            Err(EditError::NotEnoughAngles)
        ));
        let mic = MediaId(project.alloc());
        project.media.push(asset(
            mic.0,
            "lav.wav",
            300,
            Timebase::fps_24(),
            false,
            true,
        ));
        let ids = vec![project.media[0].id, project.media[1].id, mic];
        let clip_id = create_multicam(&mut project, seq, &ids, Frame(0)).unwrap();
        assert_eq!(project.multicam_groups[0].angles[0].audio, Some(mic));
        assert!(project.multicam_groups[0].angles[1].audio.is_none());
        let sequence = project.sequence(seq).unwrap();
        let audio = sequence
            .tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Audio)
            .flat_map(|track| track.clips.iter())
            .next()
            .unwrap();
        assert_eq!(audio.media_id, Some(mic));
        assert!(audio.linked.contains(&clip_id));
    }
}
