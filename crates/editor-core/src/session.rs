//! Undo/redo session around a [`Project`].
//!
//! Structural edits snapshot the whole project. Interactive slider drags snapshot
//! once on press and mutate until release.

use crate::edit::{self, EditError, TrackFlag, TrimEdge};
use crate::model::{
    BinId, Clip, ClipId, CueId, LabelColor, MarkerId, MediaAsset, MediaId, Project, SequenceId,
    TrackId, TransitionAlign, TransitionId, TransitionKind,
};
use crate::time::Frame;

#[derive(Clone, Debug)]
struct HistoryEntry {
    label: String,
    project: Project,
}

#[derive(Clone, Debug)]
pub struct Session {
    project: Project,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    dirty: bool,
    interactive: Option<String>,
    saved_generation: u64,
    generation: u64,
}

impl Session {
    pub fn new(mut project: Project) -> Self {
        project.normalize();
        Self {
            project,
            undo: Vec::new(),
            redo: Vec::new(),
            dirty: false,
            interactive: None,
            saved_generation: 0,
            generation: 0,
        }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
        self.saved_generation = self.generation;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|e| e.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|e| e.label.as_str())
    }

    pub fn replace_project(&mut self, mut project: Project) {
        project.normalize();
        self.project = project;
        self.undo.clear();
        self.redo.clear();
        self.interactive = None;
        self.dirty = false;
        self.generation = self.generation.saturating_add(1);
        self.saved_generation = self.generation;
    }

    pub fn edit(
        &mut self,
        label: &str,
        f: impl FnOnce(&mut Project) -> Result<(), EditError>,
    ) -> Result<(), EditError> {
        if self.interactive.is_some() {
            let result = f(&mut self.project);
            if result.is_ok() {
                self.project.normalize();
                self.dirty = true;
                self.generation = self.generation.saturating_add(1);
            }
            return result;
        }
        let before = self.project.clone();
        match f(&mut self.project) {
            Ok(()) => {
                self.project.normalize();
                self.push_undo(label, before);
                Ok(())
            }
            Err(error) => {
                self.project = before;
                Err(error)
            }
        }
    }

    pub fn begin_interactive(&mut self, label: &str) {
        if self.interactive.is_some() {
            return;
        }
        let before = self.project.clone();
        self.push_undo(label, before);
        self.interactive = Some(label.to_string());
    }

    pub fn end_interactive(&mut self) {
        if self.interactive.take().is_none() {
            return;
        }
        if let Some(last) = self.undo.last() {
            if last.project == self.project {
                self.undo.pop();
            }
        }
    }

    pub fn undo(&mut self) -> bool {
        self.end_interactive();
        let Some(entry) = self.undo.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.project, entry.project);
        self.redo.push(HistoryEntry {
            label: entry.label,
            project: current,
        });
        self.dirty = true;
        self.generation = self.generation.saturating_add(1);
        true
    }

    pub fn redo(&mut self) -> bool {
        self.end_interactive();
        let Some(entry) = self.redo.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.project, entry.project);
        self.undo.push(HistoryEntry {
            label: entry.label,
            project: current,
        });
        self.dirty = true;
        self.generation = self.generation.saturating_add(1);
        true
    }

    pub fn active_id(&self) -> Result<SequenceId, EditError> {
        edit::active_sequence_id(&self.project)
    }

    fn push_undo(&mut self, label: &str, before: Project) {
        self.undo.push(HistoryEntry {
            label: label.to_string(),
            project: before,
        });
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
        self.generation = self.generation.saturating_add(1);
    }
}

impl Session {
    pub fn overwrite(&mut self, clips: Vec<(TrackId, Clip)>) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Overwrite", |project| {
            edit::overwrite_clips(project, seq, clips)
        })
    }

    pub fn insert(&mut self, clips: Vec<(TrackId, Clip)>) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Insert", |project| edit::insert_clips(project, seq, clips))
    }

    pub fn razor_clip(&mut self, clip: ClipId, at: Frame) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Razor", |project| edit::razor_clip(project, seq, clip, at))
    }

    pub fn razor_at(&mut self, at: Frame) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Add edit", |project| edit::razor_at(project, seq, at))
    }

    pub fn lift_delete(&mut self, clips: Vec<ClipId>) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Lift", |project| edit::lift_delete(project, seq, &clips))
    }

    pub fn ripple_delete(&mut self, clips: Vec<ClipId>) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Ripple delete", |project| {
            edit::ripple_delete(project, seq, &clips)
        })
    }

    pub fn move_clips(
        &mut self,
        clips: Vec<ClipId>,
        delta: i64,
        retarget: Option<(ClipId, TrackId)>,
    ) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Move", |project| {
            edit::move_clips(project, seq, &clips, delta, retarget)
        })
    }

    pub fn trim(&mut self, clip: ClipId, edge: TrimEdge, delta: i64) -> Result<(), EditError> {
        let seq = self.active_id()?;
        let label = match edge {
            TrimEdge::Head => "Trim head",
            TrimEdge::Tail => "Trim tail",
        };
        self.edit(label, |project| edit::trim(project, seq, clip, edge, delta))
    }

    pub fn ripple_trim(
        &mut self,
        clip: ClipId,
        edge: TrimEdge,
        delta: i64,
    ) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Ripple", |project| {
            edit::ripple_trim(project, seq, clip, edge, delta)
        })
    }

    pub fn ripple_trim_prev_to_playhead(
        &mut self,
        at: Frame,
        tracks: &[TrackId],
    ) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Ripple trim previous", |project| {
            edit::ripple_trim_prev_to_playhead(project, seq, at, tracks)
        })
    }

    pub fn ripple_trim_next_to_playhead(
        &mut self,
        at: Frame,
        tracks: &[TrackId],
    ) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Ripple trim next", |project| {
            edit::ripple_trim_next_to_playhead(project, seq, at, tracks)
        })
    }

    pub fn roll(&mut self, left_clip: ClipId, delta: i64) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Roll", |project| {
            edit::roll_cut(project, seq, left_clip, delta)
        })
    }

    pub fn slip(&mut self, clip: ClipId, delta: i64) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Slip", |project| edit::slip(project, seq, clip, delta))
    }

    pub fn slide(&mut self, clip: ClipId, delta: i64) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Slide", |project| edit::slide(project, seq, clip, delta))
    }

    pub fn add_transition(
        &mut self,
        left_clip: ClipId,
        kind: TransitionKind,
        duration: i64,
    ) -> Result<TransitionId, EditError> {
        let seq = self.active_id()?;
        let mut id = TransitionId(0);
        self.edit("Add transition", |project| {
            id = edit::add_transition(
                project,
                seq,
                left_clip,
                kind,
                duration,
                TransitionAlign::Center,
            )?;
            Ok(())
        })?;
        Ok(id)
    }

    pub fn set_track_flag(
        &mut self,
        track: TrackId,
        flag: TrackFlag,
        value: bool,
    ) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Track", |project| {
            edit::set_track_flag(project, seq, track, flag, value)
        })
    }

    pub fn relink_media(&mut self, id: MediaId, source: MediaAsset) -> Result<(), EditError> {
        self.edit("Relink media", |project| {
            edit::relink_media(project, id, &source)
        })
    }

    pub fn attach_proxies(&mut self, links: Vec<(MediaId, String)>) -> Result<(), EditError> {
        self.edit("Attach proxies", |project| {
            edit::attach_proxies(project, &links)?;
            project.prefer_proxies = true;
            Ok(())
        })
    }

    /// Preview preference. Not an undo step; it is saved with the project.
    pub fn set_prefer_proxies(&mut self, enabled: bool) {
        if self.project.prefer_proxies == enabled {
            return;
        }
        self.project.prefer_proxies = enabled;
        self.dirty = true;
        self.generation = self.generation.saturating_add(1);
    }

    pub fn import_media(&mut self, asset: MediaAsset) -> Result<MediaId, EditError> {
        let mut id = MediaId(0);
        self.edit("Import media", |project| {
            id = edit::import_media(project, asset);
            Ok(())
        })?;
        Ok(id)
    }

    pub fn add_marker(&mut self, frame: Frame, name: &str) -> Result<(), EditError> {
        self.add_marker_with_color(frame, name, LabelColor::Amber)
            .map(|_| ())
    }

    pub fn add_marker_with_color(
        &mut self,
        frame: Frame,
        name: &str,
        color: LabelColor,
    ) -> Result<MarkerId, EditError> {
        let seq = self.active_id()?;
        let name = name.to_string();
        let mut marker_id = MarkerId(0);
        self.edit("Add marker", |project| {
            marker_id = edit::add_marker_with_color(project, seq, frame, name, color)?;
            Ok(())
        })?;
        Ok(marker_id)
    }

    pub fn update_marker(
        &mut self,
        marker_id: MarkerId,
        name: Option<String>,
        color: Option<LabelColor>,
        comment: Option<String>,
        frame: Option<Frame>,
    ) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Edit marker", |project| {
            edit::update_marker(project, seq, marker_id, name, color, comment, frame)
        })
    }

    pub fn delete_marker(&mut self, marker_id: MarkerId) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Delete marker", |project| {
            edit::delete_marker(project, seq, marker_id)
        })
    }

    pub fn create_bin(
        &mut self,
        parent: Option<BinId>,
        name: &str,
    ) -> Result<BinId, EditError> {
        let name = name.to_string();
        let mut bin_id = BinId(0);
        self.edit("Create bin", |project| {
            bin_id = edit::create_bin(project, parent, name)?;
            Ok(())
        })?;
        Ok(bin_id)
    }

    pub fn rename_bin(&mut self, bin_id: BinId, name: &str) -> Result<(), EditError> {
        let name = name.to_string();
        self.edit("Rename bin", |project| edit::rename_bin(project, bin_id, name))
    }

    pub fn delete_bin(&mut self, bin_id: BinId) -> Result<(), EditError> {
        self.edit("Delete bin", |project| edit::delete_bin(project, bin_id))
    }

    pub fn move_media_to_bin(
        &mut self,
        media_ids: &[MediaId],
        bin_id: BinId,
    ) -> Result<(), EditError> {
        let ids = media_ids.to_vec();
        self.edit("Move media", |project| edit::move_media_to_bin(project, &ids, bin_id))
    }

    pub fn set_in_point(&mut self, frame: Option<Frame>) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Mark in", |project| edit::set_in_point(project, seq, frame))
    }

    pub fn set_out_point(&mut self, frame: Option<Frame>) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Mark out", |project| {
            edit::set_out_point(project, seq, frame)
        })
    }

    pub fn update_cue_text(&mut self, cue: CueId, text: String) -> Result<(), EditError> {
        let seq = self.active_id()?;
        if self.interactive.is_some() {
            self.dirty = true;
            return edit::update_cue_text(&mut self.project, seq, cue, text);
        }
        self.edit("Edit caption", |project| {
            edit::update_cue_text(project, seq, cue, text)
        })
    }

    pub fn delete_cue(&mut self, cue: CueId) -> Result<(), EditError> {
        let seq = self.active_id()?;
        self.edit("Delete caption", |project| {
            edit::delete_cue(project, seq, cue)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Clip, Sequence, SequenceId, TrackId, TrackKind};
    use crate::time::Timebase;

    fn session() -> (Session, TrackId) {
        let mut sequence = Sequence::new(SequenceId(1), "T", 1920, 1080, Timebase::fps_24());
        let track = sequence.add_track(TrackId(2), TrackKind::Video, "V1");
        sequence.tracks[0].clips = vec![Clip::basic(1, 0, 100)];
        let mut project = Project::new("undo");
        project.sequences.push(sequence);
        project.active_sequence = Some(SequenceId(1));
        project.next_id = 10;
        (Session::new(project), track)
    }

    #[test]
    fn undo_redo_restores_overwrite() {
        let (mut session, track) = session();
        session
            .overwrite(vec![(track, Clip::basic(4, 40, 70))])
            .unwrap();
        assert_eq!(session.project().active().unwrap().tracks[0].clips.len(), 3);
        assert!(session.undo());
        assert_eq!(session.project().active().unwrap().tracks[0].clips.len(), 1);
        assert_eq!(
            session.project().active().unwrap().tracks[0].clips[0].timeline_out,
            Frame(100)
        );
        assert!(session.redo());
        assert_eq!(session.project().active().unwrap().tracks[0].clips.len(), 3);
        assert_eq!(session.undo_label(), Some("Overwrite"));
    }

    #[test]
    fn failed_edit_does_not_record_history() {
        let (mut session, _track) = session();
        let err = session.ripple_trim(ClipId(1), TrimEdge::Tail, 50);
        assert!(err.is_err());
        assert!(!session.can_undo());
        assert_eq!(
            session.project().active().unwrap().tracks[0].clips[0].timeline_out,
            Frame(100)
        );
    }
}
