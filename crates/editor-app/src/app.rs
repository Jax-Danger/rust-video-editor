//! Application state, commands, and workspace layout.

use editor_core::{
    add_transition, builtin_templates, clip_from_media, expand_linked, link_clips, plan_export,
    replace_captions, Bin, BinId, BusState, CaptionTranscriber, ClipId, CueId, Direction,
    EditError, ExportRange, Frame, MediaAsset, MediaId, Project, Session, Timebase, Track,
    TrackFlag, TrackId, TrackKind, TransitionKind, TrimEdge,
};
use editor_media::{duration_frames, probe, resolve_media_path};
use egui::{Event, Key, Modifiers, RichText, ViewportCommand};

use crate::audio::{collect_bus_pieces, collect_pieces, topology_of, AudioEngine};
use crate::dialogs;
use crate::preview::PreviewEngine;
use crate::theme;
use crate::ui::{self, format_tc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrubSource {
    Ruler,
    Viewer,
}

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
            Self::Razor => "C  ·  split at the playhead; click a clip to split it",
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
    Audio,
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

struct CaptionRequest {
    name: String,
    start: i64,
    end: i64,
    timebase: editor_core::Timebase,
    #[cfg_attr(not(all(feature = "ffmpeg", feature = "whisper")), allow(dead_code))]
    pieces: Vec<crate::audio::AudioPiece>,
}

pub struct DeliverState {
    pub codec: String,
    pub container: String,
    pub use_in_out: bool,
    pub output_path: String,
    pub report: String,
    pub burn_captions: bool,
    pub progress: f32,
}

impl Default for DeliverState {
    fn default() -> Self {
        Self {
            codec: "H.264".into(),
            container: "mp4".into(),
            use_in_out: false,
            output_path: "/tmp/meridian-export.mp4".into(),
            report: String::new(),
            burn_captions: true,
            progress: 0.0,
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
    /// Fraction of the library column given to the media pool. The rest is captions.
    pub pool_split: f32,
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
    pub preview: PreviewEngine,
    pub play_rate: i32,
    pub preview_scrub: bool,
    pub scrub: Option<ScrubSource>,
    pub reveal_playhead: bool,
    pub text_editing: bool,
    pub dragging_media: Option<MediaId>,
    pub timeline_view: Option<egui::Rect>,
    pub ruler_rect: Option<egui::Rect>,
    pub viewer_bar: Option<egui::Rect>,
    pub viewer_bar_end: i64,
    pub audio: AudioEngine,
    pub picture_cache: Option<crate::composite::PictureCache>,
    #[cfg(feature = "ffmpeg")]
    pub export_job: Option<editor_media::ExportJob>,
    #[cfg(all(feature = "ffmpeg", feature = "whisper"))]
    pub caption_job: Option<crate::caption_job::CaptionJob>,
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
            pool_split: 0.62,
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
            preview: PreviewEngine::new(),
            play_rate: 0,
            preview_scrub: false,
            scrub: None,
            reveal_playhead: false,
            text_editing: false,
            dragging_media: None,
            timeline_view: None,
            ruler_rect: None,
            viewer_bar: None,
            viewer_bar_end: 0,
            audio: AudioEngine::new(),
            picture_cache: None,
            #[cfg(feature = "ffmpeg")]
            export_job: None,
            #[cfg(all(feature = "ffmpeg", feature = "whisper"))]
            caption_job: None,
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
        self.sync_audio_bus();
        self.follow_audio_topology();
        if self.audio.meters_hot() {
            ctx.request_repaint();
        }
        if !self.playing {
            let _ = self.audio.pump();
            return;
        }
        ctx.request_repaint();
        let end = self.sequence_end();
        if self.play_rate == 1 && self.audio.drives_picture() {
            if let Some(frame) = self.audio.pump() {
                self.playhead = frame.clamp(0, end);
                self.reveal_playhead = true;
                if frame >= end {
                    self.halt_transport();
                    self.status = "End of sequence.".into();
                }
                return;
            }
            let _ = self.audio.pump();
            return;
        }
        let _ = self.audio.pump();
        let rate = if self.play_rate == 0 {
            1
        } else {
            self.play_rate
        };
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
        self.play_accum += dt * rate.unsigned_abs().max(1) as f32;
        let frame_dur = self.timebase().frame_duration_secs().max(1.0 / 120.0) as f32;
        while self.play_accum >= frame_dur {
            self.play_accum -= frame_dur;
            if rate < 0 {
                if self.playhead <= 0 {
                    self.playhead = 0;
                    self.halt_transport();
                    break;
                }
                self.playhead -= 1;
            } else {
                self.playhead += 1;
                if self.playhead >= end {
                    self.playhead = end;
                    self.halt_transport();
                    break;
                }
            }
            self.reveal_playhead = true;
        }
    }

    pub fn halt_transport(&mut self) {
        let running = self.playing || self.play_rate != 0;
        self.playing = false;
        self.play_rate = 0;
        self.play_accum = 0.0;
        if running {
            self.audio.stop();
        }
    }

    pub fn zoom_by(&mut self, factor: f32) {
        self.pixels_per_frame = (self.pixels_per_frame * factor).clamp(0.2, 64.0);
    }

    pub fn note_text_focus(&mut self, response: &egui::Response) {
        if response.has_focus() {
            self.text_editing = true;
        }
    }

    fn shuttle(&mut self, direction: i32) {
        self.audio.stop();
        if direction > 0 {
            self.play_rate = if self.play_rate > 0 {
                (self.play_rate.saturating_mul(2)).min(8)
            } else {
                1
            };
        } else {
            self.play_rate = if self.play_rate < 0 {
                (self.play_rate.saturating_mul(2)).max(-8)
            } else {
                -1
            };
        }
        self.playing = true;
        self.play_accum = 0.0;
        if self.play_rate == 1 {
            self.start_audio();
        } else {
            self.status = if self.play_rate < 0 {
                format!("Shuttle {}× (reverse is silent).", self.play_rate)
            } else {
                format!("Shuttle {}× (audio plays at 1×).", self.play_rate)
            };
        }
    }

    pub(crate) fn toggle_play(&mut self) {
        if self.playing {
            self.halt_transport();
            self.status = "Paused.".into();
            return;
        }
        self.play_rate = 1;
        self.playing = true;
        self.play_accum = 0.0;
        self.start_audio();
        if self.audio.drives_picture() {
            self.status = "Play.".into();
        } else if self.audio.badge() == "No device" || self.audio.badge() == "No audio" {
            self.status = self.audio.status().to_string();
        } else {
            self.status = "Play.".into();
        }
    }

    fn sync_audio_bus(&mut self) {
        let bus = self.session.project().active().map(BusState::from_sequence);
        if let Some(bus) = bus {
            self.audio.set_bus(bus);
        }
    }

    fn follow_audio_topology(&mut self) {
        if !(self.playing && self.play_rate == 1) {
            return;
        }
        let hash = self
            .session
            .project()
            .active()
            .map(topology_of)
            .unwrap_or(0);
        if hash != self.audio.topology() {
            self.start_audio();
        }
    }

    fn start_audio(&mut self) {
        let fps = self.timebase().fps_f64();
        let end = self.sequence_end();
        let playhead = self.playhead;
        let (pieces, hash, bus) = {
            let project = self.session.project();
            match project.active() {
                Some(sequence) => (
                    collect_bus_pieces(sequence, &project.media, playhead, end),
                    topology_of(sequence),
                    BusState::from_sequence(sequence),
                ),
                None => (
                    Vec::new(),
                    0,
                    BusState {
                        master: 1.0,
                        tracks: Vec::new(),
                        clips: Vec::new(),
                    },
                ),
            }
        };
        self.audio.set_bus(bus);
        self.audio.set_topology(hash);
        self.audio.begin(playhead, end, fps, pieces);
    }

    pub(crate) fn step_playhead(&mut self, delta: i64) {
        self.halt_transport();
        self.preview_scrub = true;
        self.reveal_playhead = true;
        self.playhead = (self.playhead + delta).max(0);
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if self.text_editing {
            return;
        }
        let mods = ctx.input(|i| i.modifiers);
        let pressed = |key: Key| consume_key(ctx, Modifiers::NONE, key, true);
        let tap = |key: Key| consume_key(ctx, Modifiers::NONE, key, false);
        let pressed_cmd = |key: Key| consume_key(ctx, Modifiers::COMMAND, key, false);
        let held_cmd = |key: Key| consume_key(ctx, Modifiers::COMMAND, key, true);
        let pressed_shift = |key: Key| consume_key(ctx, Modifiers::SHIFT, key, true);
        let pressed_cmd_shift =
            |key: Key| consume_key(ctx, Modifiers::COMMAND | Modifiers::SHIFT, key, false);
        let second = self.timebase().timecode_fps().max(1);

        if pressed_cmd_shift(Key::Z) || pressed_cmd(Key::Y) {
            if self.session.redo() {
                self.status = "Redo.".into();
            }
        } else if pressed_cmd(Key::Z) {
            if self.session.undo() {
                self.status = "Undo.".into();
            }
        } else if pressed_cmd(Key::S) {
            self.save_or_prompt();
        } else if pressed_cmd(Key::O) {
            self.open_dialog();
        } else if pressed_cmd(Key::N) {
            self.modal = Modal::NewProject {
                name: "Untitled".into(),
                template: 1,
            };
        } else if pressed_cmd(Key::I) {
            self.import_dialog();
        } else if pressed_cmd(Key::K) || (tap(Key::C) && !mods.command) {
            self.tool = Tool::Razor;
            self.split_at_playhead();
        } else if held_cmd(Key::ArrowLeft) {
            self.step_playhead(-second);
        } else if held_cmd(Key::ArrowRight) {
            self.step_playhead(second);
        } else if tap(Key::Space) {
            self.toggle_play();
        } else if tap(Key::J) {
            self.shuttle(-1);
        } else if tap(Key::K) {
            self.halt_transport();
            self.status = "Stop.".into();
        } else if tap(Key::L) {
            self.shuttle(1);
        } else if pressed_shift(Key::ArrowLeft) {
            self.step_playhead(-10);
        } else if pressed_shift(Key::ArrowRight) {
            self.step_playhead(10);
        } else if pressed(Key::ArrowLeft) {
            self.step_playhead(-1);
        } else if pressed(Key::ArrowRight) {
            self.step_playhead(1);
        } else if pressed(Key::ArrowUp) {
            self.jump_edit(-1);
        } else if pressed(Key::ArrowDown) {
            self.jump_edit(1);
        } else if pressed(Key::Home) {
            self.halt_transport();
            self.playhead = 0;
            self.reveal_playhead = true;
        } else if pressed(Key::End) {
            self.halt_transport();
            self.playhead = self.sequence_end();
            self.reveal_playhead = true;
        } else if pressed(Key::I) && !mods.command {
            self.mark_in();
        } else if pressed(Key::O) && !mods.command {
            self.mark_out();
        } else if tap(Key::M) {
            self.add_marker();
        } else if tap(Key::S) && !mods.command {
            self.snap_enabled = !self.snap_enabled;
            self.status = if self.snap_enabled {
                "Snapping on.".into()
            } else {
                "Snapping off.".into()
            };
        } else if tap(Key::V) {
            self.tool = Tool::Select;
        } else if tap(Key::B) {
            self.tool = Tool::Ripple;
        } else if tap(Key::N) && !mods.command {
            self.tool = Tool::Roll;
        } else if tap(Key::Y) && !mods.command {
            self.tool = Tool::Slip;
        } else if tap(Key::U) {
            self.tool = Tool::Slide;
        } else if pressed_shift(Key::Delete) || pressed_shift(Key::Backspace) {
            self.delete_selection(true);
        } else if pressed(Key::Delete) || pressed(Key::Backspace) {
            self.delete_selection(false);
        } else if pressed(Key::Equals) || pressed(Key::Plus) {
            self.zoom_by(1.25);
        } else if pressed(Key::Minus) {
            self.zoom_by(1.0 / 1.25);
        } else if pressed_shift(Key::Z) && !mods.command {
            self.zoom_to_fit();
            self.reveal_playhead = true;
        } else if tap(Key::Escape) {
            self.selected.clear();
            self.selected_cue = None;
            self.dragging_media = None;
            self.status = "Selection cleared.".into();
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
        self.halt_transport();
        self.reveal_playhead = true;
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
            Ok(()) => {
                if let Some(sequence) = self.session.project().active() {
                    let placed: Vec<ClipId> = sequence
                        .tracks
                        .iter()
                        .flat_map(|track| track.clips.iter())
                        .filter(|clip| {
                            clip.media_id == Some(media_id) && clip.covers(Frame(playhead))
                        })
                        .map(|clip| clip.id)
                        .collect();
                    if let Some(first) = placed.first().copied() {
                        self.selected = if self.linked_selection {
                            expand_linked(sequence, &[first])
                        } else {
                            placed
                        };
                    }
                }
                format!("{label} at {}.", format_tc(playhead, self.timebase()))
            }
            Err(err) => err.to_string(),
        };
    }

    pub fn caption_backend_note(&self) -> String {
        #[cfg(all(feature = "ffmpeg", feature = "whisper"))]
        {
            match editor_media::whisper_availability() {
                Ok(paths) => format!(
                    "Whisper · {}",
                    paths
                        .model
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("model")
                ),
                Err(err) => format!("Stub captions — {err}"),
            }
        }
        #[cfg(not(all(feature = "ffmpeg", feature = "whisper")))]
        {
            "Stub captions. Rebuild with --features whisper for local Whisper.".into()
        }
    }

    pub fn auto_caption(&mut self) {
        #[cfg(all(feature = "ffmpeg", feature = "whisper"))]
        if self
            .caption_job
            .as_ref()
            .is_some_and(|job| !job.snapshot().finished)
        {
            self.status = "Whisper is already running.".into();
            return;
        }
        let Some(request) = self.caption_request() else {
            return;
        };
        #[cfg(all(feature = "ffmpeg", feature = "whisper"))]
        if self.try_start_whisper(&request) {
            return;
        }
        self.write_stub_captions(&request);
    }

    fn caption_request(&mut self) -> Option<CaptionRequest> {
        let sequence = self.session.project().active()?;
        let (name, start, end) = if let Some(clip_id) = self.selected.first() {
            if let Some(clip) = sequence.clip(*clip_id) {
                (clip.name.clone(), clip.timeline_in.0, clip.timeline_out.0)
            } else {
                ("Sequence".into(), 0, sequence.end_frame().0)
            }
        } else if let (Some(inn), Some(out)) = (sequence.in_point, sequence.out_point) {
            ("In/Out".into(), inn.0, out.0)
        } else {
            ("Sequence".into(), 0, sequence.end_frame().0)
        };
        if end <= start {
            self.status = "Nothing to transcribe.".into();
            return None;
        }
        let media = self.session.project().media.clone();
        let pieces = collect_pieces(sequence, &media, start, end);
        Some(CaptionRequest {
            name,
            start,
            end,
            timebase: sequence.timebase,
            pieces,
        })
    }

    fn write_stub_captions(&mut self, request: &CaptionRequest) {
        let transcribe = editor_core::TranscribeRequest {
            media_name: request.name.clone(),
            language: Some("en".into()),
            range_in: Frame(request.start),
            range_out: Frame(request.end),
            timebase: request.timebase,
        };
        let drafts = match self.transcriber.transcribe(&transcribe) {
            Ok(drafts) => drafts,
            Err(err) => {
                self.status = err.to_string();
                return;
            }
        };
        self.apply_captions(drafts, "stub transcriber");
    }

    fn apply_captions(&mut self, drafts: Vec<editor_core::CaptionDraft>, engine: &str) {
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
            Ok(()) => format!("Auto caption wrote {count} cues ({engine})."),
            Err(err) => err.to_string(),
        };
    }

    #[cfg(all(feature = "ffmpeg", feature = "whisper"))]
    fn try_start_whisper(&mut self, request: &CaptionRequest) -> bool {
        if let Err(err) = editor_media::whisper_availability() {
            self.status = format!("{err} Using stub cues.");
            return false;
        }
        if request.pieces.is_empty() {
            self.status = "No audible audio in that range. Using stub cues.".into();
            return false;
        }
        let pieces = request
            .pieces
            .iter()
            .map(|piece| editor_media::WavPiece {
                path: piece.path.clone(),
                timeline_in: piece.timeline_in,
                timeline_out: piece.timeline_out,
                source_at_in: piece.source_at_in,
                seconds_per_frame: piece.seconds_per_frame,
                gain: piece.gain,
                pan: piece.pan,
                gain_keys: piece.gain_keys.clone(),
            })
            .collect();
        self.caption_job = Some(crate::caption_job::spawn_caption(
            pieces,
            request.start,
            request.end,
            request.timebase,
            request.name.clone(),
            Some("en".into()),
        ));
        self.status = "Transcribing with Whisper…".into();
        true
    }

    fn pump_jobs(&mut self, ctx: &egui::Context) {
        let _ = ctx;
        #[cfg(feature = "ffmpeg")]
        {
            let snap = self.export_job.as_ref().map(|job| job.snapshot());
            if let Some(snap) = snap {
                self.deliver.progress = snap.fraction;
                self.deliver.report = snap.message.clone();
                if snap.finished {
                    self.export_job = None;
                    self.status = if snap.ok {
                        "Export finished.".into()
                    } else {
                        snap.message
                    };
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }
        #[cfg(all(feature = "ffmpeg", feature = "whisper"))]
        {
            let snap = self.caption_job.as_ref().map(|job| job.snapshot());
            if let Some(snap) = snap {
                if snap.finished {
                    self.caption_job = None;
                    if snap.ok {
                        self.apply_captions(snap.drafts, "Whisper");
                    } else if let Some(request) = self.caption_request() {
                        self.write_stub_captions(&request);
                        self.status = format!("{}. Using stub cues.", snap.message);
                    } else {
                        self.status = format!("{}. Using stub cues.", snap.message);
                    }
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }
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
        self.pixels_per_frame = (width / frames).clamp(0.2, 64.0);
    }

    pub fn import_dialog(&mut self) {
        match dialogs::import_media_files() {
            Some(paths) => self.import_paths(&paths),
            None => self.status = "Import cancelled.".into(),
        }
    }

    pub fn open_dialog(&mut self) {
        match dialogs::open_project_file() {
            Some(path) => self.open_path(&path.to_string_lossy()),
            None => self.status = "Open cancelled.".into(),
        }
    }

    pub fn save_dialog(&mut self) {
        let name = dialogs::project_file_name(&self.session.project().name);
        let directory = self.path.as_deref().and_then(|path| {
            std::path::Path::new(path)
                .parent()
                .map(|dir| dir.to_path_buf())
        });
        match dialogs::save_project_file(&name, directory.as_deref()) {
            Some(path) => self.save_to(&path.to_string_lossy()),
            None => self.status = "Save cancelled.".into(),
        }
    }

    pub fn import_paths(&mut self, paths: &[std::path::PathBuf]) {
        let mut assets = Vec::new();
        let mut errors = Vec::new();
        for path in paths {
            if dialogs::is_project_file(path) {
                errors.push(format!(
                    "{} is a project file — use File → Open",
                    path.display()
                ));
                continue;
            }
            match probe(path.as_path()) {
                Ok(mut result) => {
                    result.path = canonical_media_path(path);
                    result.offline = ui::media_missing(&result.path);
                    assets.push(asset_from_probe(&result));
                }
                Err(err) => errors.push(format!("{}: {err}", path.display())),
            }
        }
        if assets.is_empty() {
            self.status = if errors.is_empty() {
                "Nothing to import.".into()
            } else {
                errors.join("  ")
            };
            return;
        }
        let count = assets.len();
        let result = self.session.edit("Import media", move |project| {
            ensure_master_bin(project);
            for asset in assets {
                editor_core::import_media(project, asset);
            }
            Ok(())
        });
        match result {
            Ok(()) => {
                self.selected_media = self.session.project().media.last().map(|media| media.id);
                let extra = if errors.is_empty() {
                    String::new()
                } else {
                    format!("  {}", errors.join("  "))
                };
                self.status = format!(
                    "Imported {count} file(s). Double-click, drag onto the timeline, or use Overwrite / Insert.{extra}"
                );
                self.modal = Modal::None;
            }
            Err(err) => self.status = err.to_string(),
        }
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
            self.save_dialog();
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
            Ok(mut project) => {
                refresh_offline(&mut project);
                self.session.replace_project(project);
                self.path = Some(path.to_string());
                self.playhead = 0;
                self.selected.clear();
                self.selected_media = None;
                self.halt_transport();
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
                self.halt_transport();
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
        self.halt_transport();
        self.workspace = Workspace::Edit;
        self.status = "Opened example project — Northline — Opening.".into();
    }

    pub(crate) fn start_export(&mut self) {
        #[cfg(feature = "ffmpeg")]
        if self
            .export_job
            .as_ref()
            .is_some_and(|job| !job.snapshot().finished)
        {
            self.deliver.report = "An export is already running.".into();
            return;
        }
        let Some(sequence) = self.session.project().active().cloned() else {
            self.deliver.report = "No sequence.".into();
            return;
        };
        let media = self.session.project().media.clone();
        let range = if self.deliver.use_in_out {
            ExportRange::InOut
        } else {
            ExportRange::WholeSequence
        };
        let output = self.deliver.output_path.clone();
        #[cfg(feature = "ffmpeg")]
        {
            match editor_media::plan_encode(
                &sequence,
                &media,
                range,
                &self.deliver.codec,
                &self.deliver.container,
                self.deliver.burn_captions,
            ) {
                Ok(script) => {
                    let duration = script.duration_secs;
                    match editor_media::spawn_export(script, std::path::PathBuf::from(&output)) {
                        Ok(job) => {
                            self.export_job = Some(job);
                            self.deliver.progress = 0.0;
                            self.deliver.report = format!(
                                "Encoding {output} ({duration:.2}s, {}).",
                                self.deliver.codec
                            );
                            self.status = "Export started.".into();
                        }
                        Err(err) => self.deliver.report = err,
                    }
                }
                Err(err) => self.deliver.report = err,
            }
        }
        #[cfg(not(feature = "ffmpeg"))]
        {
            let _ = (sequence, media, range, output);
            self.deliver.report = "Picture encoding needs the ffmpeg feature. Rebuild with --features ffmpeg, then Export writes an H.264/AAC mp4.".into();
            self.export_manifest();
        }
    }

    pub(crate) fn export_running(&self) -> bool {
        #[cfg(feature = "ffmpeg")]
        {
            self.export_job.is_some()
        }
        #[cfg(not(feature = "ffmpeg"))]
        {
            false
        }
    }

    pub(crate) fn cancel_export(&mut self) {
        #[cfg(feature = "ffmpeg")]
        if let Some(job) = &self.export_job {
            job.cancel();
            self.deliver.report = "Cancelling export…".into();
            self.status = "Cancelling export.".into();
        }
        #[cfg(not(feature = "ffmpeg"))]
        {
            self.deliver.report = "No encode is running.".into();
        }
    }

    #[cfg_attr(feature = "ffmpeg", allow(dead_code))]
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

fn ensure_master_bin(project: &mut Project) {
    if project.bins.is_empty() {
        let id = BinId(project.alloc());
        project.bins.push(Bin {
            id,
            name: "Master".into(),
            parent: None,
        });
    }
}

fn canonical_media_path(path: &std::path::Path) -> String {
    let raw = path.to_string_lossy();
    let resolved = resolve_media_path(&raw);
    let target = if resolved.is_file() {
        resolved
    } else {
        path.to_path_buf()
    };
    std::fs::canonicalize(&target)
        .unwrap_or(target)
        .to_string_lossy()
        .into_owned()
}

fn refresh_offline(project: &mut Project) {
    for media in &mut project.media {
        media.offline = ui::media_missing(&media.path);
    }
}

fn consume_key(ctx: &egui::Context, modifiers: Modifiers, key: Key, allow_repeat: bool) -> bool {
    ctx.input_mut(|input| {
        let mut matched = false;
        input.events.retain(|event| {
            let Event::Key {
                key: ev_key,
                modifiers: ev_mods,
                pressed: true,
                repeat,
                ..
            } = event
            else {
                return true;
            };
            if *ev_key != key || !ev_mods.matches_logically(modifiers) {
                return true;
            }
            if *repeat && !allow_repeat {
                return false;
            }
            matched = true;
            false
        });
        matched
    })
}

impl eframe::App for MeridianApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.preview_scrub = false;
        self.pump_jobs(ctx);
        self.tick_playback(ctx);
        self.handle_keys(ctx);
        self.text_editing = false;
        let dropped: Vec<std::path::PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            self.import_paths(&dropped);
        }
        self.sync_title(ctx);
        self.menu_bar(ctx);
        self.toolbar(ctx);
        self.status_bar(ctx);

        let (timeline_h, pool_w, inspector_w) = shell_sizes(ctx);
        let max_timeline = (ctx.screen_rect().height() * 0.68).max(360.0);
        egui::TopBottomPanel::bottom("timeline")
            .resizable(true)
            .default_height(timeline_h)
            .height_range(220.0..=max_timeline)
            .frame(theme::panel_frame())
            .show_separator_line(true)
            .show(ctx, |ui| {
                ui::timeline_panel(ui, self);
            });

        match self.workspace {
            Workspace::Edit => {
                egui::SidePanel::left("library")
                    .resizable(true)
                    .default_width(pool_w)
                    .width_range(220.0..=440.0)
                    .frame(theme::panel_frame())
                    .show_separator_line(true)
                    .show(ctx, |ui| library_column(ui, self));
                egui::SidePanel::right("inspector")
                    .resizable(true)
                    .default_width(inspector_w)
                    .width_range(260.0..=480.0)
                    .frame(theme::panel_frame())
                    .show_separator_line(true)
                    .show(ctx, |ui| ui::inspector_panel(ui, self));
                egui::CentralPanel::default()
                    .frame(theme::chrome_frame().fill(theme::THEME.stage))
                    .show(ctx, |ui| ui::viewer_panel(ui, self));
            }
            Workspace::Colour => {
                egui::SidePanel::left("colour_scopes")
                    .resizable(true)
                    .default_width(248.0)
                    .width_range(180.0..=400.0)
                    .frame(theme::panel_frame())
                    .show_separator_line(true)
                    .show(ctx, |ui| ui::scopes_panel(ui, self));
                egui::SidePanel::right("colour_inspector")
                    .resizable(true)
                    .default_width((inspector_w + 56.0).min(460.0))
                    .width_range(300.0..=540.0)
                    .frame(theme::panel_frame())
                    .show_separator_line(true)
                    .show(ctx, |ui| ui::inspector_panel(ui, self));
                egui::CentralPanel::default()
                    .frame(theme::chrome_frame().fill(theme::THEME.stage))
                    .show(ctx, |ui| ui::viewer_panel(ui, self));
            }
            Workspace::Audio => {
                egui::CentralPanel::default()
                    .frame(theme::panel_frame())
                    .show(ctx, |ui| ui::audio_workspace(ui, self));
            }
            Workspace::Deliver => {
                egui::CentralPanel::default()
                    .frame(theme::chrome_frame().fill(theme::THEME.stage))
                    .show(ctx, |ui| ui::deliver_panel(ui, self));
            }
        }

        self.modals(ctx);
        if ctx.input(|input| input.pointer.any_released()) {
            self.dragging_media = None;
        }
    }
}

impl MeridianApp {
    fn menu_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menu")
            .frame(theme::chrome_frame())
            .exact_height(theme::MENU_H)
            .show_separator_line(false)
            .show(ctx, |ui| {
                let bar = ui.max_rect();
                ui.painter()
                    .hline(bar.x_range(), bar.bottom(), theme::hairline_stroke());
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
                        if ui.button("Open…    Ctrl+O").clicked() {
                            self.open_dialog();
                            ui.close_menu();
                        }
                        if ui.button("Open Example").clicked() {
                            self.open_example();
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Save    Ctrl+S").clicked() {
                            self.save_or_prompt();
                            ui.close_menu();
                        }
                        if ui.button("Save As…").clicked() {
                            self.save_dialog();
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Import Media…    Ctrl+I").clicked() {
                            self.import_dialog();
                            ui.close_menu();
                        }
                        if ui.button("Import from Path…").clicked() {
                            open_import(self);
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Deliver / Export").clicked() {
                            self.workspace = Workspace::Deliver;
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
            .exact_height(theme::TOOLBAR_H)
            .show_separator_line(true)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.add_space(theme::SPACE_SM);
                    let tool = self.tool;
                    for (id, shortcut, active, hint, icon) in [
                        (
                            "select",
                            "V",
                            tool == Tool::Select,
                            Tool::Select.hint(),
                            ui::widgets::paint_select as fn(&egui::Painter, egui::Rect),
                        ),
                        (
                            "razor",
                            "C",
                            tool == Tool::Razor,
                            Tool::Razor.hint(),
                            ui::widgets::paint_razor,
                        ),
                        (
                            "ripple",
                            "B",
                            tool == Tool::Ripple,
                            Tool::Ripple.hint(),
                            ui::widgets::paint_ripple,
                        ),
                        (
                            "roll",
                            "N",
                            tool == Tool::Roll,
                            Tool::Roll.hint(),
                            ui::widgets::paint_roll,
                        ),
                        (
                            "slip",
                            "Y",
                            tool == Tool::Slip,
                            Tool::Slip.hint(),
                            ui::widgets::paint_slip,
                        ),
                        (
                            "slide",
                            "U",
                            tool == Tool::Slide,
                            Tool::Slide.hint(),
                            ui::widgets::paint_slide,
                        ),
                    ] {
                        if ui::widgets::tool_cell(ui, id, shortcut, active, hint, icon) {
                            self.tool = match shortcut {
                                "C" => Tool::Razor,
                                "B" => Tool::Ripple,
                                "N" => Tool::Roll,
                                "Y" => Tool::Slip,
                                "U" => Tool::Slide,
                                _ => Tool::Select,
                            };
                        }
                    }
                    ui::widgets::v_hairline(ui, theme::TOOLBAR_H);
                    ui.add_space(theme::SPACE_SM);
                    if ui::widgets::chip(ui, "Snap", self.snap_enabled) {
                        self.snap_enabled = !self.snap_enabled;
                    }
                    ui.add_space(theme::SPACE_XS);
                    if ui::widgets::chip(ui, "Linked", self.linked_selection) {
                        self.linked_selection = !self.linked_selection;
                    }
                    ui::widgets::v_hairline(ui, theme::TOOLBAR_H);
                    ui.add_space(theme::SPACE_SM);
                    if ui::widgets::action_button(ui, "Overwrite", true) {
                        self.place_selected_media(false);
                    }
                    ui.add_space(theme::SPACE_XS);
                    if ui::widgets::action_button(ui, "Insert", false) {
                        self.place_selected_media(true);
                    }
                    ui.add_space(theme::SPACE_MD);
                    ui.label(
                        RichText::new("NUDGE")
                            .font(theme::THEME.font(9.0))
                            .color(theme::THEME.text_mute),
                    );
                    for step in [-5_i64, -1, 1, 5] {
                        if ui::widgets::ghost_button(ui, &format!("{step:+}")) {
                            self.nudge = step;
                            self.nudge_tool(step);
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(theme::SPACE_MD);
                        ui.label(
                            RichText::new(self.tool.hint())
                                .font(theme::THEME.font(11.0))
                                .color(theme::THEME.text_mute),
                        );
                    });
                });
            });
    }

    fn status_bar(&self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(theme::STATUS_H)
            .frame(theme::chrome_frame())
            .show_separator_line(false)
            .show(ctx, |ui| {
                let bar = ui.max_rect();
                ui.painter()
                    .hline(bar.x_range(), bar.top(), theme::hairline_stroke());
                ui.horizontal(|ui| {
                    ui.add_space(theme::SPACE_MD);
                    let sequence = self.session.project().active();
                    let res = sequence
                        .map(|s| format!("{}×{}", s.width, s.height))
                        .unwrap_or_else(|| "—".into());
                    let fps = sequence
                        .map(|s| format!("{:.3} fps", s.timebase.fps_f64()))
                        .unwrap_or_default();
                    let page = match self.workspace {
                        Workspace::Edit => "Edit",
                        Workspace::Colour => "Colour",
                        Workspace::Audio => "Audio",
                        Workspace::Deliver => "Deliver",
                    };
                    ui.label(
                        RichText::new(page)
                            .font(theme::THEME.font(11.0))
                            .color(theme::THEME.text_dim),
                    );
                    ui.label(
                        RichText::new(self.tool.label())
                            .font(theme::THEME.font(11.0))
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
                    let response = ui.add(egui::TextEdit::singleline(&mut name).desired_width(280.0));
                    self.note_text_focus(&response);
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
                let response = ui.add(egui::TextEdit::singleline(&mut path).desired_width(420.0));
                self.note_text_focus(&response);
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
                ui.label("With --features ffmpeg, Probe calls ffprobe and the program viewer decodes frames with ffmpeg. Otherwise files are classified by extension and the viewer draws placeholders.");
                let response = ui.add(egui::TextEdit::singleline(&mut path).desired_width(440.0));
                self.note_text_focus(&response);
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
                ui.set_min_width(520.0);
                ui.set_max_height(520.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (key, action) in SHORTCUTS {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(*key).monospace().color(theme::AMBER));
                            ui.label(*action);
                        });
                    }
                });
            });
        if !shown {
            self.modal = Modal::None;
        }
    }
}

const SHORTCUTS: &[(&str, &str)] = &[
    ("Space", "Play / pause at 1×"),
    (
        "J  K  L",
        "Reverse shuttle, stop, forward shuttle (tap again to go faster)",
    ),
    ("Left / Right", "Step one frame"),
    ("Shift+Left / Right", "Jump 10 frames"),
    ("Ctrl+Left / Right", "Jump one second"),
    ("Up / Down", "Previous / next edit"),
    ("Home / End", "Go to start / end"),
    ("I / O", "Mark in / out"),
    ("M", "Add marker"),
    ("V", "Select tool"),
    ("C  /  Ctrl+K", "Razor at the playhead"),
    ("B  N  Y  U", "Ripple, roll, slip, slide tools"),
    ("S", "Toggle snapping"),
    ("Delete / Backspace", "Lift delete"),
    ("Shift+Delete", "Ripple delete"),
    ("Esc", "Clear selection"),
    ("Ctrl+Z", "Undo"),
    ("Ctrl+Shift+Z  /  Ctrl+Y", "Redo"),
    ("Ctrl+S", "Save"),
    ("Ctrl+O", "Open project"),
    ("Ctrl+N", "New project"),
    ("Ctrl+I", "Import media"),
    ("+ / −", "Zoom timeline"),
    ("Ctrl+scroll  /  pinch", "Zoom timeline"),
    ("Scroll", "Pan timeline"),
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

fn shell_sizes(ctx: &egui::Context) -> (f32, f32, f32) {
    let rect = ctx.screen_rect();
    let timeline = (rect.height() * 0.42).clamp(300.0, 520.0);
    let pool = (rect.width() * 0.20).clamp(260.0, 360.0);
    let inspector = (rect.width() * 0.22).clamp(280.0, 400.0);
    (timeline, pool, inspector)
}

fn library_column(ui: &mut egui::Ui, app: &mut MeridianApp) {
    let total = ui.available_height();
    let handle = 6.0;
    let usable = (total - handle).max(1.0);
    let min_pool = 120.0;
    let min_cap = 108.0;
    let pool_h = if usable <= min_pool + min_cap {
        usable * app.pool_split.clamp(0.35, 0.75)
    } else {
        (usable * app.pool_split).clamp(min_pool, usable - min_cap)
    };
    ui.allocate_ui(egui::vec2(ui.available_width(), pool_h), |ui| {
        ui::media_pool(ui, app);
    });
    let delta = ui::widgets::h_split(ui);
    if delta.abs() > 0.0 {
        app.pool_split = ((pool_h + delta) / usable).clamp(0.28, 0.78);
    }
    ui::captions_panel(ui, app);
}

fn workspace_switch(ui: &mut egui::Ui, workspace: &mut Workspace) {
    let labels = ["Edit", "Colour", "Audio", "Deliver"];
    let selected = match *workspace {
        Workspace::Edit => 0,
        Workspace::Colour => 1,
        Workspace::Audio => 2,
        Workspace::Deliver => 3,
    };
    if let Some(index) = ui::widgets::workspace_modes(ui, selected, &labels) {
        *workspace = match index {
            0 => Workspace::Edit,
            1 => Workspace::Colour,
            2 => Workspace::Audio,
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
        path: String::new(),
        note: "Paste a path, or use File → Import for the system dialog.".into(),
    };
}
