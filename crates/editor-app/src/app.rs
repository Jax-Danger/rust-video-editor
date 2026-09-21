//! Application state, commands, and workspace layout.

use editor_core::{
    add_transition, builtin_templates, clip_from_media, expand_linked, link_clips, plan_export,
    replace_captions, BinId, CaptionTranscriber, ClipId, CueId, Direction, EditError, ExportRange,
    Frame, MediaAsset, MediaId, Project, Session, Timebase, Track, TrackFlag, TrackId, TrackKind,
    TransitionKind, TrimEdge,
};
use editor_media::{duration_frames, probe};
use egui::{Key, Modifiers, RichText, ViewportCommand};

use crate::theme;
use crate::ui::{self, format_tc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Razor,
    Ripple,
    Roll,
    Slip,
    Slide,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Razor => "Razor",
            Self::Ripple => "Ripple",
            Self::Roll => "Roll",
            Self::Slip => "Slip",
            Self::Slide => "Slide",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Select => "V  ·  drag a clip to move, drag an edge to trim",
            Self::Razor => "C  ·  click a clip to split it and its linked pair",
            Self::Ripple => "B  ·  drag an edge; downstream clips follow",
            Self::Roll => "N  ·  drag a cut; the neighbour absorbs the change",
            Self::Slip => "Y  ·  drag to shift source without moving the clip",
            Self::Slide => "U  ·  drag between neighbours; outer span stays put",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Workspace {
    Edit,
    Colour,
    Deliver,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragKind {
    Move,
    TrimHead,
    TrimTail,
    RippleHead,
    RippleTail,
    RollHead,
    RollTail,
    Slip,
    Slide,
}

#[derive(Clone, Debug)]
pub struct Drag {
    pub kind: DragKind,
    pub clip_id: ClipId,
    pub track_id: TrackId,
    pub origin_in: i64,
    pub origin_out: i64,
    pub press_x: f32,
    pub current_x: f32,
    pub ppf: f32,
}

impl Drag {
    pub fn delta_frames(&self) -> i64 {
        if self.ppf <= 0.0 {
            0
        } else {
            ((self.current_x - self.press_x) / self.ppf).round() as i64
        }
    }
}

#[derive(Clone, Debug)]
pub enum Modal {
    None,
    NewProject { name: String, template: usize },
    Open { path: String, error: String },
    SaveAs { path: String, error: String },
    Import { path: String, note: String },
    Shortcuts,
}

pub struct DeliverState {
    pub codec: String,
    pub container: String,
    pub use_in_out: bool,
    pub output_path: String,
    pub report: String,
}

impl Default for DeliverState {
    fn default() -> Self {
        Self {
            codec: "H.264".into(),
            container: "mp4".into(),
            use_in_out: false,
            output_path: "/tmp/meridian-export.json".into(),
            report: String::new(),
        }
    }
}

pub struct MeridianApp {
    pub session: Session,
    pub playhead: i64,
    pub selected: Vec<ClipId>,
    pub selected_media: Option<MediaId>,
    pub selected_cue: Option<CueId>,
    pub tool: Tool,
    pub linked_selection: bool,
    pub snap_enabled: bool,
    pub workspace: Workspace,
    pub pixels_per_frame: f32,
    pub timeline_width: f32,
    pub playing: bool,
    pub play_accum: f32,
    pub status: String,
    pub path: Option<String>,
    pub modal: Modal,
    pub deliver: DeliverState,
    pub drag: Option<Drag>,
    pub nudge: i64,
    pub transcriber: editor_core::StubTranscriber,
}

impl MeridianApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let app = Self {
            session: Session::new(editor_core::demo_project()),
            playhead: 24,
            selected: vec![ClipId(301)],
            selected_media: Some(MediaId(10)),
            selected_cue: None,
            tool: Tool::Select,
            linked_selection: true,
            snap_enabled: true,
            workspace: Workspace::Edit,
            pixels_per_frame: 4.0,
            timeline_width: 900.0,
            playing: false,
            play_accum: 0.0,
            status: "Opened example project — Northline — Opening.".into(),
            path: None,
            modal: Modal::None,
            deliver: DeliverState::default(),
            drag: None,
            nudge: 1,
            transcriber: editor_core::StubTranscriber::default(),
        };
        app.sync_title(&cc.egui_ctx);
        app
    }

    pub fn sequence_end(&self) -> i64 {
        self.session
            .project()
            .active()
            .map(|s| s.end_frame().0)
            .unwrap_or(0)
    }

    pub fn timebase(&self) -> Timebase {
        self.session
            .project()
            .active()
            .map(|s| s.timebase)
            .unwrap_or_else(Timebase::fps_24)
    }

    pub fn selected_with_links(&self) -> Vec<ClipId> {
        let Some(sequence) = self.session.project().active() else {
            return Vec::new();
        };
        if self.linked_selection {
            expand_linked(sequence, &self.selected)
        } else {
            self.selected.clone()
        }
    }

    fn sync_title(&self, ctx: &egui::Context) {
        let name = &self.session.project().name;
        let dirty = if self.session.is_dirty() { " •" } else { "" };
        ctx.send_viewport_cmd(ViewportCommand::Title(format!("Meridian — {name}{dirty}")));
    }

    fn tick_playback(&mut self, ctx: &egui::Context) {
        if !self.playing {
            return;
        }
        ctx.request_repaint();
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
        self.play_accum += dt;
        let frame_dur = self.timebase().frame_duration_secs().max(1.0 / 120.0) as f32;
        let end = self.sequence_end();
        while self.play_accum >= frame_dur {
            self.play_accum -= frame_dur;
            self.playhead += 1;
            if self.playhead >= end {
                self.playhead = end;
                self.playing = false;
                self.play_accum = 0.0;
                break;
            }
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let mods = ctx.input(|i| i.modifiers);
        let pressed = |key: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key));
        let pressed_cmd = |key: Key| ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, key));
        let pressed_shift = |key: Key| ctx.input_mut(|i| i.consume_key(Modifiers::SHIFT, key));
        let pressed_cmd_shift =
            |key: Key| ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, key));

        if pressed_cmd(Key::Z) {
            if self.session.undo() {
                self.status = "Undo.".into();
            }
        } else if pressed_cmd_shift(Key::Z) || pressed_cmd(Key::Y) {
            if self.session.redo() {
                self.status = "Redo.".into();
            }
        } else if pressed_cmd(Key::S) {
            self.save_or_prompt();
        } else if pressed(Key::Space) {
            self.playing = !self.playing;
            self.play_accum = 0.0;
        } else if pressed(Key::ArrowLeft) {
            self.playing = false;
            self.playhead = (self.playhead - 1).max(0);
        } else if pressed(Key::ArrowRight) {
            self.playing = false;
            self.playhead += 1;
        } else if pressed_shift(Key::ArrowLeft) {
            self.playing = false;
            self.playhead = (self.playhead - self.timebase().timecode_fps()).max(0);
        } else if pressed_shift(Key::ArrowRight) {
            self.playing = false;
            self.playhead += self.timebase().timecode_fps();
        } else if pressed(Key::ArrowUp) {
            self.jump_edit(-1);
        } else if pressed(Key::ArrowDown) {
            self.jump_edit(1);
        } else if pressed(Key::Home) {
            self.playhead = 0;
            self.playing = false;
        } else if pressed(Key::End) {
            self.playhead = self.sequence_end();
            self.playing = false;
        } else if pressed(Key::I) {
            self.mark_in();
        } else if pressed(Key::O) {
            self.mark_out();
        } else if pressed(Key::M) {
            self.add_marker();
        } else if pressed(Key::S) && !mods.command {
            self.snap_enabled = !self.snap_enabled;
            self.status = if self.snap_enabled {
                "Snapping on.".into()
            } else {
                "Snapping off.".into()
            };
        } else if pressed(Key::V) {
            self.tool = Tool::Select;
        } else if pressed(Key::C) && !mods.command {
            self.tool = Tool::Razor;
        } else if pressed(Key::B) {
            self.tool = Tool::Ripple;
        } else if pressed(Key::N) {
            self.tool = Tool::Roll;
        } else if pressed(Key::Y) && !mods.command {
            self.tool = Tool::Slip;
        } else if pressed(Key::U) {
            self.tool = Tool::Slide;
        } else if pressed(Key::Delete) || pressed(Key::Backspace) {
            self.delete_selection(false);
        } else if pressed_shift(Key::Delete) || pressed_shift(Key::Backspace) {
            self.delete_selection(true);
        } else if pressed_cmd(Key::K) {
            self.split_at_playhead();
        } else if pressed(Key::Equals) || pressed(Key::Plus) {
            self.pixels_per_frame = (self.pixels_per_frame * 1.25).min(24.0);
        } else if pressed(Key::Minus) {
            self.pixels_per_frame = (self.pixels_per_frame / 1.25).max(0.35);
        } else if pressed_shift(Key::Z) && !mods.command {
            self.zoom_to_fit();
        }
    }

    fn jump_edit(&mut self, direction: i64) {
        let Some(sequence) = self.session.project().active() else {
            return;
        };
        let points = sequence.edit_points();
        if direction > 0 {
            if let Some(frame) = points.iter().find(|f| **f > self.playhead) {
                self.playhead = *frame;
            }
        } else if let Some(frame) = points.iter().rev().find(|f| **f < self.playhead) {
            self.playhead = *frame;
        }
        self.playing = false;
    }

    pub fn mark_in(&mut self) {
        let frame = Frame(self.playhead);
        if let Err(err) = self.session.set_in_point(Some(frame)) {
            self.status = err.to_string();
        } else {
            self.status = format!("In {}", format_tc(self.playhead, self.timebase()));
        }
    }

    pub fn mark_out(&mut self) {
        let frame = Frame(self.playhead);
        if let Err(err) = self.session.set_out_point(Some(frame)) {
            self.status = err.to_string();
        } else {
            self.status = format!("Out {}", format_tc(self.playhead, self.timebase()));
        }
    }

    pub fn clear_marks(&mut self) {
        let _ = self.session.set_in_point(None);
        let _ = self.session.set_out_point(None);
        self.status = "Cleared in and out.".into();
    }

    pub fn add_marker(&mut self) {
        let name = format!("Marker {}", format_tc(self.playhead, self.timebase()));
        match self.session.add_marker(Frame(self.playhead), &name) {
            Ok(()) => self.status = format!("Added {name}."),
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn split_at_playhead(&mut self) {
        match self.session.razor_at(Frame(self.playhead)) {
            Ok(()) => self.status = "Split clips under the playhead.".into(),
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn delete_selection(&mut self, ripple: bool) {
        let ids = self.selected_with_links();
        if ids.is_empty() {
            self.status = "Nothing selected.".into();
            return;
        }
        let result = if ripple {
            self.session.ripple_delete(ids)
        } else {
            self.session.lift_delete(ids)
        };
        match result {
            Ok(()) => {
                self.selected.clear();
                self.status = if ripple {
                    "Ripple delete.".into()
                } else {
                    "Lifted selection.".into()
                };
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn nudge_tool(&mut self, delta: i64) {
        let Some(clip_id) = self.selected.first().copied() else {
            self.status = "Select a clip.".into();
            return;
        };
        let result = match self.tool {
            Tool::Select => self.session.trim(clip_id, TrimEdge::Tail, delta),
            Tool::Razor => {
                self.status = "Razor has no nudge — click the clip.".into();
                return;
            }
            Tool::Ripple => self.session.ripple_trim(clip_id, TrimEdge::Tail, delta),
            Tool::Roll => self.session.roll(clip_id, delta),
            Tool::Slip => self.session.slip(clip_id, delta),
            Tool::Slide => self.session.slide(clip_id, delta),
        };
        match result {
            Ok(()) => self.status = format!("{} {delta:+}", self.tool.label()),
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn place_selected_media(&mut self, insert: bool) {
        let Some(media_id) = self.selected_media else {
            self.status = "Select a clip in the media pool.".into();
            return;
        };
        let playhead = self.playhead.max(0);
        let label = if insert { "Insert" } else { "Overwrite" };
        let result = self.session.edit(label, |project| {
            place_media(project, media_id, playhead, insert)
        });
        self.status = match result {
            Ok(()) => format!("{label} at {}.", format_tc(playhead, self.timebase())),
            Err(err) => err.to_string(),
        };
    }

    pub fn auto_caption(&mut self) {
        let request = {
            let Some(sequence) = self.session.project().active() else {
                self.status = "No sequence.".into();
                return;
            };
            let (name, start, end) = if let Some(clip_id) = self.selected.first() {
                if let Some(clip) = sequence.clip(*clip_id) {
                    (clip.name.clone(), clip.timeline_in, clip.timeline_out)
                } else {
                    ("Sequence".into(), Frame(0), sequence.end_frame())
                }
            } else {
                ("Sequence".into(), Frame(0), sequence.end_frame())
            };
            if end.0 <= start.0 {
                self.status = "Nothing to transcribe.".into();
                return;
            }
            editor_core::TranscribeRequest {
                media_name: name,
                language: Some("en".into()),
                range_in: start,
                range_out: end,
                timebase: sequence.timebase,
            }
        };
        let drafts = match self.transcriber.transcribe(&request) {
            Ok(drafts) => drafts,
            Err(err) => {
                self.status = err.to_string();
                return;
            }
        };
        let count = drafts.len();
        let result = self.session.edit("Auto caption", |project| {
            let seq_id = project.active_sequence.ok_or(EditError::NoActiveSequence)?;
            let track_id = {
                let sequence = project
                    .sequence(seq_id)
                    .ok_or(EditError::SequenceNotFound)?;
                if let Some(track) = sequence
                    .tracks
                    .iter()
                    .find(|t| t.kind == TrackKind::Caption)
                {
                    track.id
                } else {
                    let id = TrackId(project.alloc());
                    project
                        .sequence_mut(seq_id)
                        .unwrap()
                        .tracks
                        .push(Track::new(id, TrackKind::Caption, "C1"));
                    id
                }
            };
            replace_captions(project, seq_id, track_id, drafts)
        });
        self.status = match result {
            Ok(()) => format!("Auto caption wrote {count} cues (stub transcriber)."),
            Err(err) => err.to_string(),
        };
    }

    pub fn add_transition_at_selection(&mut self, kind: TransitionKind) {
        let Some(clip_id) = self.selected.first().copied() else {
            self.status = "Select the clip on the left side of the cut.".into();
            return;
        };
        let result = self.session.edit("Add transition", |project| {
            let seq = project.active_sequence.ok_or(EditError::NoActiveSequence)?;
            add_transition(
                project,
                seq,
                clip_id,
                kind,
                12,
                editor_core::TransitionAlign::Center,
            )?;
            Ok(())
        });
        self.status = match result {
            Ok(()) => "Added transition on the outgoing cut.".into(),
            Err(err) => err.to_string(),
        };
    }

    pub fn add_track(&mut self, kind: TrackKind) {
        let result = self.session.edit("Add track", |project| {
            let seq_id = project.active_sequence.ok_or(EditError::NoActiveSequence)?;
            let count = project
                .sequence(seq_id)
                .ok_or(EditError::SequenceNotFound)?
                .tracks
                .iter()
                .filter(|t| t.kind == kind)
                .count();
            let id = TrackId(project.alloc());
            project
                .sequence_mut(seq_id)
                .ok_or(EditError::SequenceNotFound)?
                .tracks
                .push(Track::new(
                    id,
                    kind,
                    format!("{}{}", kind.prefix(), count + 1),
                ));
            Ok(())
        });
        self.status = match result {
            Ok(()) => format!("Added {} track.", kind.prefix()),
            Err(err) => err.to_string(),
        };
    }

    pub fn zoom_to_fit(&mut self) {
        let frames = (self.sequence_end() + 24).max(48) as f32;
        let width = self.timeline_width.max(200.0);
        self.pixels_per_frame = (width / frames).clamp(0.35, 24.0);
    }

    pub fn import_path(&mut self, path: &str) {
        let path = path.trim();
        if path.is_empty() {
            self.status = "Enter a media path.".into();
            return;
        }
        match probe(std::path::Path::new(path)) {
            Ok(result) => {
                let note = format!(
                    "{}  {}×{}  {}  {}",
                    result
                        .video_codec
                        .as_deref()
                        .or(result.audio_codec.as_deref())
                        .unwrap_or("media"),
                    result.width.unwrap_or(0),
                    result.height.unwrap_or(0),
                    if result.offline { "offline" } else { "online" },
                    if result.has_video && result.has_audio {
                        "A/V"
                    } else if result.has_video {
                        "video"
                    } else {
                        "audio"
                    }
                );
                let asset = asset_from_probe(&result);
                match self.session.import_media(asset) {
                    Ok(id) => {
                        self.selected_media = Some(id);
                        self.status = format!("Imported {path} — {note}.");
                        self.modal = Modal::None;
                    }
                    Err(err) => self.status = err.to_string(),
                }
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    fn save_or_prompt(&mut self) {
        if let Some(path) = self.path.clone() {
            self.save_to(&path);
        } else {
            let suggested = suggested_path(&self.session.project().name);
            self.modal = Modal::SaveAs {
                path: suggested,
                error: String::new(),
            };
        }
    }

    pub fn save_to(&mut self, path: &str) {
        match self.session.project().save_file(std::path::Path::new(path)) {
            Ok(()) => {
                self.session.mark_clean();
                self.path = Some(path.to_string());
                self.status = format!("Saved {path}.");
                self.modal = Modal::None;
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn open_path(&mut self, path: &str) {
        match Project::load_file(std::path::Path::new(path)) {
            Ok(project) => {
                self.session.replace_project(project);
                self.path = Some(path.to_string());
                self.playhead = 0;
                self.selected.clear();
                self.playing = false;
                self.status = format!("Opened {path}.");
                self.modal = Modal::None;
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn new_from_template(&mut self, index: usize, name: &str) {
        let templates = builtin_templates();
        let Some(template) = templates.get(index) else {
            return;
        };
        let name = if name.trim().is_empty() {
            template.name.clone()
        } else {
            name.trim().to_string()
        };
        match editor_core::project_from_template(template, &name) {
            Ok(project) => {
                self.session.replace_project(project);
                self.path = None;
                self.playhead = 0;
                self.selected.clear();
                self.selected_media = None;
                self.playing = false;
                self.workspace = Workspace::Edit;
                self.status = format!("New project from {}.", template.name);
                self.modal = Modal::None;
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn open_example(&mut self) {
        self.session.replace_project(editor_core::demo_project());
        self.path = None;
        self.playhead = 24;
        self.selected = vec![ClipId(301)];
        self.selected_media = Some(MediaId(10));
        self.playing = false;
        self.workspace = Workspace::Edit;
        self.status = "Opened example project — Northline — Opening.".into();
    }

    pub(crate) fn export_manifest(&mut self) {
        let Some(sequence) = self.session.project().active() else {
            self.deliver.report = "No sequence.".into();
            return;
        };
        let range = if self.deliver.use_in_out {
            ExportRange::InOut
        } else {
            ExportRange::WholeSequence
        };
        let plan = plan_export(
            sequence,
            &self.deliver.codec,
            &self.deliver.container,
            &self.deliver.output_path,
            range,
        );
        match serde_json::to_string_pretty(&plan) {
            Ok(json) => {
                if let Err(err) = std::fs::write(&self.deliver.output_path, json.as_bytes()) {
                    self.deliver.report = format!("Could not write manifest: {err}");
                } else {
                    self.deliver.report = format!(
                        "Wrote export manifest to {}.\nPicture encoding is not linked in this build — the manifest is the deliverable a later encoder consumes.\n{}–{}  {}  {} clips.",
                        self.deliver.output_path,
                        format_tc(plan.in_frame, sequence.timebase),
                        format_tc(plan.out_frame, sequence.timebase),
                        plan.codec,
                        plan.video_clips + plan.audio_clips
                    );
                    self.status = "Export manifest written.".into();
                }
            }
            Err(err) => self.deliver.report = err.to_string(),
        }
    }
}

fn place_media(
    project: &mut Project,
    media_id: MediaId,
    playhead: i64,
    insert: bool,
) -> Result<(), EditError> {
    let seq_id = project.active_sequence.ok_or(EditError::NoActiveSequence)?;
    let timebase = project
        .sequence(seq_id)
        .ok_or(EditError::SequenceNotFound)?
        .timebase;
    let media = project
        .media(media_id)
        .cloned()
        .ok_or(EditError::MediaNotFound)?;
    let video_track = project
        .sequence(seq_id)
        .unwrap()
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Video && !t.locked)
        .map(|t| t.id);
    let audio_track = project
        .sequence(seq_id)
        .unwrap()
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Audio && !t.locked)
        .map(|t| t.id);
    let mut video = None;
    let mut audio = None;
    if media.has_video {
        let track = video_track.ok_or(EditError::WrongTrackKind)?;
        let id = ClipId(project.alloc());
        video = Some((
            track,
            clip_from_media(
                id,
                &media,
                timebase,
                Frame(playhead),
                Frame::ZERO,
                media.duration,
                media.name.clone(),
            )?,
        ));
    }
    if media.has_audio {
        let track = audio_track.ok_or(EditError::WrongTrackKind)?;
        let id = ClipId(project.alloc());
        audio = Some((
            track,
            clip_from_media(
                id,
                &media,
                timebase,
                Frame(playhead),
                Frame::ZERO,
                media.duration,
                media.name.clone(),
            )?,
        ));
    }
    if video.is_none() && audio.is_none() {
        return Err(EditError::MediaNotFound);
    }
    if let (Some((_, video_clip)), Some((_, audio_clip))) = (&mut video, &mut audio) {
        audio_clip.timeline_in = video_clip.timeline_in;
        audio_clip.timeline_out = video_clip.timeline_out;
        link_clips(video_clip, audio_clip);
    }
    let mut clips = Vec::new();
    if let Some(pair) = video {
        clips.push(pair);
    }
    if let Some(pair) = audio {
        clips.push(pair);
    }
    if insert {
        editor_core::insert_clips(project, seq_id, clips)
    } else {
        editor_core::overwrite_clips(project, seq_id, clips)
    }
}

fn asset_from_probe(result: &editor_media::ProbeResult) -> MediaAsset {
    let timebase = result.timebase.unwrap_or_else(Timebase::fps_24);
    let frames = duration_frames(result, timebase);
    let name = std::path::Path::new(&result.path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("media")
        .to_string();
    MediaAsset {
        id: MediaId(0),
        bin_id: BinId(0),
        name,
        path: result.path.clone(),
        duration: Frame(frames),
        timebase,
        width: result.width,
        height: result.height,
        video_codec: result.video_codec.clone(),
        audio_codec: result.audio_codec.clone(),
        audio_channels: result.audio_channels,
        sample_rate: result.sample_rate,
        has_video: result.has_video,
        has_audio: result.has_audio,
        offline: result.offline,
    }
}

fn suggested_path(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("/tmp/{slug}.meridian.json")
}

impl eframe::App for MeridianApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.tick_playback(ctx);
        self.handle_keys(ctx);
        self.sync_title(ctx);
        self.menu_bar(ctx);
        self.toolbar(ctx);
        self.status_bar(ctx);

        egui::TopBottomPanel::bottom("timeline")
            .resizable(true)
            .default_height(360.0)
            .min_height(220.0)
            .frame(theme::panel_frame())
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui::timeline_panel(ui, self);
            });

        match self.workspace {
            Workspace::Edit => {
                egui::SidePanel::left("library")
                    .resizable(true)
                    .default_width(300.0)
                    .width_range(240.0..=460.0)
                    .frame(theme::panel_frame())
                    .show_separator_line(false)
                    .show(ctx, |ui| {
                        let height = ui.available_height();
                        ui.allocate_ui(egui::vec2(ui.available_width(), height * 0.56), |ui| {
                            ui::media_pool(ui, self);
                        });
                        ui::widgets::hairline(ui);
                        ui::captions_panel(ui, self);
                    });
                egui::SidePanel::right("inspector")
                    .resizable(true)
                    .default_width(332.0)
                    .width_range(280.0..=480.0)
                    .frame(theme::panel_frame())
                    .show_separator_line(false)
                    .show(ctx, |ui| ui::inspector_panel(ui, self));
                egui::CentralPanel::default()
                    .frame(theme::chrome_frame().fill(theme::THEME.stage))
                    .show(ctx, |ui| ui::viewer_panel(ui, self));
            }
            Workspace::Colour => {
                egui::SidePanel::right("colour_inspector")
                    .resizable(true)
                    .default_width(380.0)
                    .width_range(300.0..=520.0)
                    .frame(theme::panel_frame())
                    .show_separator_line(false)
                    .show(ctx, |ui| ui::inspector_panel(ui, self));
                egui::CentralPanel::default()
                    .frame(theme::chrome_frame().fill(theme::THEME.stage))
                    .show(ctx, |ui| ui::viewer_panel(ui, self));
            }
            Workspace::Deliver => {
                egui::CentralPanel::default()
                    .frame(theme::chrome_frame().fill(theme::THEME.stage))
                    .show(ctx, |ui| ui::deliver_panel(ui, self));
            }
        }

        self.modals(ctx);
    }
}

impl MeridianApp {
    fn menu_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menu")
            .frame(theme::chrome_frame())
            .exact_height(36.0)
            .show_separator_line(false)
            .show(ctx, |ui| {
                egui::menu::bar(ui, |ui| {
                    brand_mark(ui);
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Meridian")
                            .strong()
                            .size(13.0)
                            .color(theme::THEME.text),
                    );
                    ui.add_space(10.0);
                    ui.menu_button("File", |ui| {
                        if ui.button("New Project…").clicked() {
                            self.modal = Modal::NewProject {
                                name: "Untitled".into(),
                                template: 1,
                            };
                            ui.close_menu();
                        }
                        if ui.button("Open…").clicked() {
                            self.modal = Modal::Open {
                                path: self.path.clone().unwrap_or_else(|| "/tmp/".into()),
                                error: String::new(),
                            };
                            ui.close_menu();
                        }
                        if ui.button("Open Example").clicked() {
                            self.open_example();
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Save").clicked() {
                            self.save_or_prompt();
                            ui.close_menu();
                        }
                        if ui.button("Save As…").clicked() {
                            self.modal = Modal::SaveAs {
                                path: self.path.clone().unwrap_or_else(|| {
                                    suggested_path(&self.session.project().name)
                                }),
                                error: String::new(),
                            };
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Quit").clicked() {
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                        }
                    });
                    ui.menu_button("Edit", |ui| {
                        let undo = self
                            .session
                            .undo_label()
                            .map(|l| format!("Undo {l}"))
                            .unwrap_or_else(|| "Undo".into());
                        if ui
                            .add_enabled(self.session.can_undo(), egui::Button::new(undo))
                            .clicked()
                        {
                            self.session.undo();
                            ui.close_menu();
                        }
                        let redo = self
                            .session
                            .redo_label()
                            .map(|l| format!("Redo {l}"))
                            .unwrap_or_else(|| "Redo".into());
                        if ui
                            .add_enabled(self.session.can_redo(), egui::Button::new(redo))
                            .clicked()
                        {
                            self.session.redo();
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Lift Delete").clicked() {
                            self.delete_selection(false);
                            ui.close_menu();
                        }
                        if ui.button("Ripple Delete").clicked() {
                            self.delete_selection(true);
                            ui.close_menu();
                        }
                        if ui.button("Split at Playhead").clicked() {
                            self.split_at_playhead();
                            ui.close_menu();
                        }
                    });
                    ui.menu_button("Sequence", |ui| {
                        if ui.button("Mark In").clicked() {
                            self.mark_in();
                            ui.close_menu();
                        }
                        if ui.button("Mark Out").clicked() {
                            self.mark_out();
                            ui.close_menu();
                        }
                        if ui.button("Clear In/Out").clicked() {
                            self.clear_marks();
                            ui.close_menu();
                        }
                        if ui.button("Add Marker").clicked() {
                            self.add_marker();
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Add Video Track").clicked() {
                            self.add_track(TrackKind::Video);
                            ui.close_menu();
                        }
                        if ui.button("Add Audio Track").clicked() {
                            self.add_track(TrackKind::Audio);
                            ui.close_menu();
                        }
                        if ui.button("Add Caption Track").clicked() {
                            self.add_track(TrackKind::Caption);
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Cross Dissolve at Cut").clicked() {
                            self.add_transition_at_selection(TransitionKind::CrossDissolve);
                            ui.close_menu();
                        }
                        if ui.button("Wipe at Cut").clicked() {
                            self.add_transition_at_selection(TransitionKind::Wipe {
                                angle_deg: 90.0,
                            });
                            ui.close_menu();
                        }
                        if ui.button("Push Slide at Cut").clicked() {
                            self.add_transition_at_selection(TransitionKind::PushSlide {
                                direction: Direction::Left,
                            });
                            ui.close_menu();
                        }
                    });
                    ui.menu_button("Help", |ui| {
                        if ui.button("Keyboard Shortcuts").clicked() {
                            self.modal = Modal::Shortcuts;
                            ui.close_menu();
                        }
                        if ui.button("About Meridian").clicked() {
                            self.status =
                                "Meridian 0.1 — frame-accurate editorial and finishing.".into();
                            ui.close_menu();
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(10.0);
                        workspace_switch(ui, &mut self.workspace);
                    });
                });
            });
    }

    fn toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar")
            .frame(theme::chrome_frame().fill(theme::THEME.panel))
            .exact_height(52.0)
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    ui.add_space(6.0);
                    let tool = self.tool;
                    if ui::widgets::tool_cell(
                        ui,
                        "select",
                        "Select",
                        tool == Tool::Select,
                        Tool::Select.hint(),
                        ui::widgets::paint_select,
                    ) {
                        self.tool = Tool::Select;
                    }
                    if ui::widgets::tool_cell(
                        ui,
                        "razor",
                        "Razor",
                        tool == Tool::Razor,
                        Tool::Razor.hint(),
                        ui::widgets::paint_razor,
                    ) {
                        self.tool = Tool::Razor;
                    }
                    if ui::widgets::tool_cell(
                        ui,
                        "ripple",
                        "Ripple",
                        tool == Tool::Ripple,
                        Tool::Ripple.hint(),
                        ui::widgets::paint_ripple,
                    ) {
                        self.tool = Tool::Ripple;
                    }
                    if ui::widgets::tool_cell(
                        ui,
                        "roll",
                        "Roll",
                        tool == Tool::Roll,
                        Tool::Roll.hint(),
                        ui::widgets::paint_roll,
                    ) {
                        self.tool = Tool::Roll;
                    }
                    if ui::widgets::tool_cell(
                        ui,
                        "slip",
                        "Slip",
                        tool == Tool::Slip,
                        Tool::Slip.hint(),
                        ui::widgets::paint_slip,
                    ) {
                        self.tool = Tool::Slip;
                    }
                    if ui::widgets::tool_cell(
                        ui,
                        "slide",
                        "Slide",
                        tool == Tool::Slide,
                        Tool::Slide.hint(),
                        ui::widgets::paint_slide,
                    ) {
                        self.tool = Tool::Slide;
                    }
                    ui.add_space(8.0);
                    if ui::widgets::chip(ui, "Snap", self.snap_enabled) {
                        self.snap_enabled = !self.snap_enabled;
                    }
                    if ui::widgets::chip(ui, "Linked", self.linked_selection) {
                        self.linked_selection = !self.linked_selection;
                    }
                    ui.add_space(8.0);
                    if ui::widgets::action_button(ui, "Overwrite", true) {
                        self.place_selected_media(false);
                    }
                    if ui::widgets::action_button(ui, "Insert", false) {
                        self.place_selected_media(true);
                    }
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Nudge")
                            .size(11.0)
                            .color(theme::THEME.text_mute),
                    );
                    for step in [-5_i64, -1, 1, 5] {
                        if ui::widgets::ghost_button(ui, &format!("{step:+}")) {
                            self.nudge = step;
                            self.nudge_tool(step);
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(self.tool.hint())
                                .size(11.0)
                                .color(theme::THEME.text_mute),
                        );
                    });
                });
            });
    }

    fn status_bar(&self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(26.0)
            .frame(theme::chrome_frame())
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    let sequence = self.session.project().active();
                    let res = sequence
                        .map(|s| format!("{}×{}", s.width, s.height))
                        .unwrap_or_else(|| "—".into());
                    let fps = sequence
                        .map(|s| format!("{:.3} fps", s.timebase.fps_f64()))
                        .unwrap_or_default();
                    ui.label(
                        RichText::new(self.tool.label())
                            .size(11.0)
                            .color(theme::THEME.accent),
                    );
                    ui.label(
                        RichText::new(if self.snap_enabled { "Snap" } else { "Free" })
                            .size(11.0)
                            .color(theme::THEME.text_mute),
                    );
                    ui.label(
                        RichText::new(&self.status)
                            .size(11.0)
                            .color(theme::THEME.text_dim),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        if self.session.is_dirty() {
                            ui.label(
                                RichText::new("Unsaved")
                                    .size(11.0)
                                    .color(theme::THEME.amber),
                            );
                        }
                        ui.label(
                            RichText::new(format!("{res}   {fps}"))
                                .size(11.0)
                                .monospace()
                                .color(theme::THEME.text_dim),
                        );
                        ui.label(
                            RichText::new(format!("{:.1} px/f", self.pixels_per_frame))
                                .size(11.0)
                                .monospace()
                                .color(theme::THEME.text_mute),
                        );
                    });
                });
            });
    }

    fn modals(&mut self, ctx: &egui::Context) {
        match self.modal.clone() {
            Modal::None => {}
            Modal::NewProject { name, template } => self.modal_new(ctx, name, template),
            Modal::Open { path, error } => self.modal_path(ctx, true, path, error),
            Modal::SaveAs { path, error } => self.modal_path(ctx, false, path, error),
            Modal::Import { path, note } => self.modal_import(ctx, path, note),
            Modal::Shortcuts => self.modal_shortcuts(ctx),
        }
    }

    fn modal_new(&mut self, ctx: &egui::Context, mut name: String, mut template: usize) {
        let mut open = true;
        let templates = builtin_templates();
        let dirty = self.session.is_dirty();
        egui::Window::new("New Project")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(theme::dialog_frame())
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(460.0);
                ui.label("Choose a template. Resolution, frame rate, and default tracks come from the preset.");
                if dirty {
                    ui.label(
                        RichText::new("The open project has unsaved changes and will be replaced.")
                            .small()
                            .color(theme::AMBER),
                    );
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut name).desired_width(280.0));
                });
                ui.add_space(4.0);
                for (index, preset) in templates.iter().enumerate() {
                    let meta = format!(
                        "{}×{}   {:.3} fps   V{} A{} C{}",
                        preset.width,
                        preset.height,
                        Timebase::new(preset.fps_num, preset.fps_den).fps_f64(),
                        preset.video_tracks,
                        preset.audio_tracks,
                        preset.caption_tracks
                    );
                    if ui::widgets::choice_card(ui, &preset.name, &meta, template == index).clicked()
                    {
                        template = index;
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui::widgets::action_button(ui, "Create", true) {
                        self.new_from_template(template, &name);
                    }
                    if ui::widgets::action_button(ui, "Cancel", false) {
                        self.modal = Modal::None;
                    }
                });
            });
        if !open {
            self.modal = Modal::None;
        } else if matches!(self.modal, Modal::NewProject { .. }) {
            self.modal = Modal::NewProject { name, template };
        }
    }

    fn modal_path(
        &mut self,
        ctx: &egui::Context,
        open_mode: bool,
        mut path: String,
        error: String,
    ) {
        let mut shown = true;
        let title = if open_mode {
            "Open Project"
        } else {
            "Save Project"
        };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(theme::dialog_frame())
            .open(&mut shown)
            .show(ctx, |ui| {
                ui.set_min_width(460.0);
                ui.label("Project files are JSON.");
                ui.add(egui::TextEdit::singleline(&mut path).desired_width(420.0));
                if !error.is_empty() {
                    ui.label(RichText::new(&error).color(theme::DANGER));
                }
                ui.horizontal(|ui| {
                    if ui.button(if open_mode { "Open" } else { "Save" }).clicked() {
                        if open_mode {
                            self.open_path(&path);
                            if self.status.starts_with("Opened") {
                                return;
                            }
                            self.modal = Modal::Open {
                                path: path.clone(),
                                error: self.status.clone(),
                            };
                        } else {
                            self.save_to(&path);
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.modal = Modal::None;
                    }
                });
            });
        if shown && !matches!(self.modal, Modal::None) {
            if open_mode {
                if matches!(self.modal, Modal::Open { .. }) {
                    self.modal = Modal::Open { path, error };
                }
            } else if matches!(self.modal, Modal::SaveAs { .. }) {
                self.modal = Modal::SaveAs { path, error };
            }
        } else if !shown {
            self.modal = Modal::None;
        }
    }

    fn modal_import(&mut self, ctx: &egui::Context, mut path: String, mut note: String) {
        let mut shown = true;
        egui::Window::new("Import Media")
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(theme::dialog_frame())
            .open(&mut shown)
            .show(ctx, |ui| {
                ui.set_min_width(480.0);
                ui.label("Probe uses the built-in stub unless Meridian was built with the ffmpeg feature. Missing files import as offline.");
                ui.add(egui::TextEdit::singleline(&mut path).desired_width(440.0));
                ui.horizontal(|ui| {
                    if ui.button("Probe").clicked() {
                        match probe(std::path::Path::new(path.trim())) {
                            Ok(result) => {
                                note = format!(
                                    "{}  {}×{}  video {}  audio {}  {}",
                                    result
                                        .timebase
                                        .map(|tb| format!("{:.3} fps", tb.fps_f64()))
                                        .unwrap_or_else(|| "rate unknown".into()),
                                    result.width.unwrap_or(0),
                                    result.height.unwrap_or(0),
                                    result.video_codec.as_deref().unwrap_or("—"),
                                    result.audio_codec.as_deref().unwrap_or("—"),
                                    if result.offline { "offline" } else { "online" }
                                );
                            }
                            Err(err) => note = err.to_string(),
                        }
                    }
                    if ui.button("Add to Pool").clicked() {
                        self.import_path(&path);
                    }
                    if ui.button("Close").clicked() {
                        self.modal = Modal::None;
                    }
                });
                if !note.is_empty() {
                    ui.label(RichText::new(&note).small().color(theme::DIM));
                }
            });
        if shown && matches!(self.modal, Modal::Import { .. }) {
            self.modal = Modal::Import { path, note };
        } else if !shown {
            self.modal = Modal::None;
        }
    }

    fn modal_shortcuts(&mut self, ctx: &egui::Context) {
        let mut shown = true;
        egui::Window::new("Keyboard Shortcuts")
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(theme::dialog_frame())
            .open(&mut shown)
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                for (key, action) in SHORTCUTS {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(*key).monospace().color(theme::AMBER));
                        ui.label(*action);
                    });
                }
            });
        if !shown {
            self.modal = Modal::None;
        }
    }
}

const SHORTCUTS: &[(&str, &str)] = &[
    ("Space", "Play / pause"),
    ("Left / Right", "Step one frame"),
    ("Shift+Left / Right", "Step one second"),
    ("Up / Down", "Previous / next edit"),
    ("Home / End", "Go to start / end"),
    ("I / O", "Mark in / out"),
    ("M", "Add marker"),
    ("V C B N Y U", "Select, razor, ripple, roll, slip, slide"),
    ("S", "Toggle snapping"),
    ("Delete", "Lift delete"),
    ("Shift+Delete", "Ripple delete"),
    ("Ctrl+K", "Split at playhead"),
    ("Ctrl+Z / Ctrl+Shift+Z", "Undo / redo"),
    ("Ctrl+S", "Save"),
    ("+ / −", "Zoom timeline"),
    ("Shift+Z", "Zoom timeline to fit"),
];

fn brand_mark(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, theme::THEME.accent);
    painter.hline(
        (rect.left() + 3.0)..=(rect.right() - 3.0),
        rect.center().y,
        egui::Stroke::new(1.6_f32, theme::THEME.accent_text),
    );
}

fn workspace_switch(ui: &mut egui::Ui, workspace: &mut Workspace) {
    let labels = ["Edit", "Colour", "Deliver"];
    let selected = match *workspace {
        Workspace::Edit => 0,
        Workspace::Colour => 1,
        Workspace::Deliver => 2,
    };
    if let Some(index) = ui::widgets::workspace_modes(ui, selected, &labels) {
        *workspace = match index {
            0 => Workspace::Edit,
            1 => Workspace::Colour,
            _ => Workspace::Deliver,
        };
    }
}

pub fn note_track_flag(app: &mut MeridianApp, track: TrackId, flag: TrackFlag, value: bool) {
    if let Err(err) = app.session.set_track_flag(track, flag, value) {
        app.status = err.to_string();
    }
}

pub fn open_import(app: &mut MeridianApp) {
    app.modal = Modal::Import {
        path: "media/interview.mov".into(),
        note: String::new(),
    };
}
