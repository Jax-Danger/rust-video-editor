//! Application state, commands, and workspace layout.

use std::collections::{HashMap, HashSet};

use editor_core::{
    add_adjustment_layer, add_title, add_transition, builtin_templates, clip_from_media,
    create_multicam, create_nested_sequence, expand_linked, link_clips, multicam_target,
    nested_sequence_id, plan_export, replace_captions, resolve_source_marks, set_angle_sync,
    switch_angle, Bin, BinId, BusState, CaptionTranscriber, ClipId, CueId, Direction, EditError,
    ExportRange, Frame, LabelColor, MarkerId, MediaAsset, MediaId, MulticamId, Project,
    SequenceId, Session, Timebase, Track, TrackFlag, TrackId, TrackKind, TransitionKind,
    TrimEdge,
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
    Source,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonitorFocus {
    Source,
    Program,
}

#[derive(Clone, Debug, Default)]
pub struct SourceMarks {
    pub playhead: i64,
    pub in_point: Option<i64>,
    pub out_point: Option<i64>,
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
    pub preset_id: String,
    pub codec: String,
    pub container: String,
    pub use_in_out: bool,
    pub output_path: String,
    pub report: String,
    pub burn_captions: bool,
    pub progress: f32,
    pub video_bitrate_kbps: Option<u32>,
    pub audio_bitrate_kbps: Option<u32>,
    pub audio_only: bool,
    pub save_preset_name: String,
    pub save_preset_note: String,
}

impl DeliverState {
    pub fn from_settings(settings: editor_core::DeliverSettings) -> Self {
        Self {
            preset_id: settings.preset_id,
            codec: settings.codec,
            container: settings.container,
            use_in_out: settings.use_in_out,
            output_path: settings.output_path,
            report: String::new(),
            burn_captions: settings.burn_captions,
            progress: 0.0,
            video_bitrate_kbps: settings.video_bitrate_kbps,
            audio_bitrate_kbps: settings.audio_bitrate_kbps,
            audio_only: settings.audio_only,
            save_preset_name: String::new(),
            save_preset_note: String::new(),
        }
    }

    pub fn to_settings(&self) -> editor_core::DeliverSettings {
        editor_core::DeliverSettings {
            preset_id: self.preset_id.clone(),
            codec: self.codec.clone(),
            container: self.container.clone(),
            use_in_out: self.use_in_out,
            burn_captions: self.burn_captions,
            output_path: self.output_path.clone(),
            video_bitrate_kbps: self.video_bitrate_kbps,
            audio_bitrate_kbps: self.audio_bitrate_kbps,
            audio_only: self.audio_only,
        }
    }

    #[cfg_attr(not(feature = "ffmpeg"), allow(dead_code))]
    pub fn encode_hints(&self) -> editor_media::EncodeHints {
        editor_media::EncodeHints {
            video_bitrate_kbps: self.video_bitrate_kbps,
            audio_bitrate_kbps: self.audio_bitrate_kbps,
            audio_only: self.audio_only,
        }
    }
}

impl Default for DeliverState {
    fn default() -> Self {
        Self::from_settings(editor_core::load_last_deliver_settings().unwrap_or_default())
    }
}

pub struct MeridianApp {
    pub session: Session,
    pub playhead: i64,
    pub selected: Vec<ClipId>,
    pub selected_media: Option<MediaId>,
    /// Pool items chosen for a multicam. Plain click replaces this; Shift adds;
    /// Ctrl toggles. [`Self::selected_media`] stays the primary item.
    pub pool_selection: Vec<MediaId>,
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
    /// Frame at the left edge of the timeline viewport. Scrolling changes this,
    /// not a pixel strip as wide as the sequence.
    pub timeline_origin: f64,
    pub ruler_rect: Option<egui::Rect>,
    pub viewer_bar: Option<egui::Rect>,
    pub viewer_bar_end: i64,
    pub source_bar: Option<egui::Rect>,
    pub source_bar_end: i64,
    /// Per-pool-item source playhead and in/out marks (media timebase).
    pub source_marks: HashMap<MediaId, SourceMarks>,
    pub focused_monitor: MonitorFocus,
    pub source_playing: bool,
    pub source_play_rate: i32,
    pub source_play_accum: f32,
    pub audio: AudioEngine,
    pub picture_cache: Option<crate::composite::PictureCache>,
    pub proxy_job: Option<crate::proxy_job::ProxyJob>,
    pub proxy_note: String,
    #[cfg(feature = "ffmpeg")]
    pub export_job: Option<editor_media::ExportJob>,
    #[cfg(all(feature = "ffmpeg", feature = "whisper"))]
    pub caption_job: Option<crate::caption_job::CaptionJob>,
    /// Video and audio tracks armed for overwrite, insert, and ripple trims.
    pub targeted_tracks: HashSet<TrackId>,
    /// Parent sequences when editing inside a nested compound clip.
    pub sequence_nav_stack: Vec<SequenceId>,
    pub selected_bin: Option<BinId>,
    pub selected_marker: Option<MarkerId>,
    /// Inline rename buffer for a media-pool bin.
    pub renaming_bin: Option<(BinId, String)>,
}

impl MeridianApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let mut app = Self {
            session: Session::new(editor_core::demo_project()),
            playhead: 24,
            selected: vec![ClipId(301)],
            selected_media: Some(MediaId(10)),
            pool_selection: vec![MediaId(10)],
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
            timeline_origin: 0.0,
            ruler_rect: None,
            viewer_bar: None,
            viewer_bar_end: 0,
            source_bar: None,
            source_bar_end: 0,
            source_marks: HashMap::new(),
            focused_monitor: MonitorFocus::Program,
            source_playing: false,
            source_play_rate: 0,
            source_play_accum: 0.0,
            audio: AudioEngine::new(),
            picture_cache: None,
            proxy_job: None,
            proxy_note: String::new(),
            #[cfg(feature = "ffmpeg")]
            export_job: None,
            #[cfg(all(feature = "ffmpeg", feature = "whisper"))]
            caption_job: None,
            targeted_tracks: HashSet::new(),
            sequence_nav_stack: Vec::new(),
            selected_bin: None,
            selected_marker: None,
            renaming_bin: None,
        };
        app.reset_track_targets();
        app.sync_title(&cc.egui_ctx);
        app
    }

    pub fn reset_track_targets(&mut self) {
        self.targeted_tracks = self
            .session
            .project()
            .active()
            .map(|sequence| {
                sequence
                    .tracks
                    .iter()
                    .filter(|track| {
                        track.kind == TrackKind::Video || track.kind == TrackKind::Audio
                    })
                    .map(|track| track.id)
                    .collect()
            })
            .unwrap_or_default();
    }

    pub fn is_track_targeted(&self, track: TrackId) -> bool {
        self.targeted_tracks.contains(&track)
    }

    pub fn toggle_track_target(&mut self, track: TrackId) {
        let name = self
            .session
            .project()
            .active()
            .and_then(|sequence| sequence.track(track).map(|t| t.name.clone()))
            .unwrap_or_else(|| "Track".into());
        if self.targeted_tracks.contains(&track) {
            self.targeted_tracks.remove(&track);
            self.status = format!("{name} target off.");
        } else {
            self.targeted_tracks.insert(track);
            self.status = format!("{name} target on.");
        }
    }

    fn toggle_video_target(&mut self, index: u32) {
        let ids = self
            .session
            .project()
            .active()
            .map(|sequence| {
                sequence
                    .tracks
                    .iter()
                    .filter(|track| track.kind == TrackKind::Video)
                    .map(|track| track.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let idx = index.saturating_sub(1) as usize;
        if let Some(id) = ids.get(idx) {
            self.toggle_track_target(*id);
        }
    }

    fn toggle_audio_target(&mut self, index: u32) {
        let ids = self
            .session
            .project()
            .active()
            .map(|sequence| {
                sequence
                    .tracks
                    .iter()
                    .filter(|track| track.kind == TrackKind::Audio)
                    .map(|track| track.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let idx = index.saturating_sub(1) as usize;
        if let Some(id) = ids.get(idx) {
            self.toggle_track_target(*id);
        }
    }

    fn targeted_track_ids(&self) -> Vec<TrackId> {
        self.targeted_tracks.iter().copied().collect()
    }

    pub fn ripple_trim_prev_to_playhead(&mut self) {
        let tracks = self.targeted_track_ids();
        match self
            .session
            .ripple_trim_prev_to_playhead(Frame(self.playhead), &tracks)
        {
            Ok(()) => self.status = "Ripple trim previous edit to playhead.".into(),
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn ripple_trim_next_to_playhead(&mut self) {
        let tracks = self.targeted_track_ids();
        match self
            .session
            .ripple_trim_next_to_playhead(Frame(self.playhead), &tracks)
        {
            Ok(()) => self.status = "Ripple trim next edit to playhead.".into(),
            Err(err) => self.status = err.to_string(),
        }
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
        self.tick_source_playback(ctx);
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

    fn tick_source_playback(&mut self, ctx: &egui::Context) {
        if !self.source_playing {
            return;
        }
        let Some(media_id) = self.selected_media else {
            self.halt_source_transport();
            return;
        };
        let Some(media) = self.session.project().media(media_id).cloned() else {
            self.halt_source_transport();
            return;
        };
        ctx.request_repaint();
        let end = media.duration.0.max(0);
        let rate = if self.source_play_rate == 0 {
            1
        } else {
            self.source_play_rate
        };
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
        self.source_play_accum += dt * rate.unsigned_abs().max(1) as f32;
        let frame_dur = media.timebase.frame_duration_secs().max(1.0 / 120.0) as f32;
        while self.source_play_accum >= frame_dur {
            self.source_play_accum -= frame_dur;
            let marks = self.source_marks.entry(media_id).or_default();
            if rate < 0 {
                if marks.playhead <= 0 {
                    marks.playhead = 0;
                    self.halt_source_transport();
                    break;
                }
                marks.playhead -= 1;
            } else {
                marks.playhead += 1;
                if marks.playhead >= end {
                    marks.playhead = end;
                    self.halt_source_transport();
                    break;
                }
            }
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

    pub fn halt_source_transport(&mut self) {
        self.source_playing = false;
        self.source_play_rate = 0;
        self.source_play_accum = 0.0;
    }

    pub fn focus_monitor(&mut self, focus: MonitorFocus) {
        if self.focused_monitor == focus {
            return;
        }
        self.focused_monitor = focus;
        match focus {
            MonitorFocus::Source => {
                self.halt_transport();
                self.status = "Source monitor focused. \\ toggles.".into();
            }
            MonitorFocus::Program => {
                self.halt_source_transport();
                self.status = "Program monitor focused. \\ toggles.".into();
            }
        }
    }

    pub fn toggle_monitor_focus(&mut self) {
        match self.focused_monitor {
            MonitorFocus::Source => self.focus_monitor(MonitorFocus::Program),
            MonitorFocus::Program => self.focus_monitor(MonitorFocus::Source),
        }
    }

    pub fn open_in_source(&mut self, media_id: MediaId) {
        self.selected_media = Some(media_id);
        if !self.pool_selection.contains(&media_id) {
            self.pool_selection = vec![media_id];
        }
        self.source_marks.entry(media_id).or_default();
        self.focus_monitor(MonitorFocus::Source);
        let name = self
            .session
            .project()
            .media(media_id)
            .map(|media| media.name.clone())
            .unwrap_or_else(|| "clip".into());
        self.status = format!("Opened {name} in source.");
    }

    pub fn source_marks_for(&self, media_id: MediaId) -> SourceMarks {
        self.source_marks
            .get(&media_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_source_playhead(&mut self, frame: i64) {
        let Some(media_id) = self.selected_media else {
            return;
        };
        let end = self
            .session
            .project()
            .media(media_id)
            .map(|media| media.duration.0.max(0))
            .unwrap_or(0);
        let marks = self.source_marks.entry(media_id).or_default();
        marks.playhead = frame.clamp(0, end);
    }

    fn source_edit_range(&self, media: &MediaAsset) -> Result<(Frame, Frame), EditError> {
        let marks = self.source_marks_for(media.id);
        resolve_source_marks(
            media.duration,
            marks.in_point.map(Frame),
            marks.out_point.map(Frame),
        )
    }

    pub fn zoom_by(&mut self, factor: f32) {
        let old = self.pixels_per_frame;
        let new = editor_core::clamp_timeline_zoom(old * factor);
        self.rebase_timeline_zoom(old, new, self.zoom_anchor_px());
    }

    /// Pixel in the timeline view that should stay on the same frame while zooming.
    /// The playhead wins when it is on screen. Otherwise the anchor is the center
    /// of the clips actually visible, not the empty area past the last clip.
    pub fn zoom_anchor_px(&self) -> f32 {
        let width = self
            .timeline_view
            .map(|rect| rect.width())
            .unwrap_or(900.0)
            .max(1.0);
        let ppf = f64::from(self.pixels_per_frame.max(editor_core::MIN_PIXELS_PER_FRAME));
        let playhead_x = (self.playhead as f64 - self.timeline_origin) * ppf;
        if playhead_x >= 0.0 && playhead_x <= f64::from(width) {
            return playhead_x as f32;
        }
        let end = self.sequence_end().max(0) as f64;
        let content_right = ((end - self.timeline_origin).max(0.0) * ppf) as f32;
        (width * 0.5).min(content_right.max(0.0))
    }

    /// Keep `anchor_px` (distance from the left of the timeline view) on the same frame.
    pub fn rebase_timeline_zoom(&mut self, old: f32, new: f32, anchor_px: f32) {
        let width = self.timeline_view.map(|rect| rect.width()).unwrap_or(900.0);
        self.pixels_per_frame = new;
        self.timeline_origin = editor_core::zoom_origin(
            self.timeline_origin,
            old,
            new,
            anchor_px,
            self.sequence_end(),
            width,
        );
    }

    pub fn clamp_timeline_origin(&mut self, end: i64) {
        let width = self
            .timeline_view
            .map(|rect| rect.width())
            .unwrap_or(900.0)
            .max(80.0) as f64;
        let ppf = f64::from(self.pixels_per_frame.max(editor_core::MIN_PIXELS_PER_FRAME));
        let view_frames = width / ppf;
        let max_origin = (end as f64 - view_frames).max(0.0);
        if !self.timeline_origin.is_finite() || self.timeline_origin < 0.0 {
            self.timeline_origin = 0.0;
        }
        if self.timeline_origin > max_origin {
            self.timeline_origin = max_origin;
        }
    }

    pub fn note_text_focus(&mut self, response: &egui::Response) {
        if response.has_focus() {
            self.text_editing = true;
        }
    }

    fn shuttle(&mut self, direction: i32) {
        if self.focused_monitor == MonitorFocus::Source {
            self.shuttle_source(direction);
            return;
        }
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

    fn shuttle_source(&mut self, direction: i32) {
        self.halt_transport();
        if direction > 0 {
            self.source_play_rate = if self.source_play_rate > 0 {
                (self.source_play_rate.saturating_mul(2)).min(8)
            } else {
                1
            };
        } else {
            self.source_play_rate = if self.source_play_rate < 0 {
                (self.source_play_rate.saturating_mul(2)).max(-8)
            } else {
                -1
            };
        }
        self.source_playing = true;
        self.source_play_accum = 0.0;
        self.status = format!("Source shuttle {}×.", self.source_play_rate);
    }

    pub(crate) fn toggle_play(&mut self) {
        if self.focused_monitor == MonitorFocus::Source {
            self.toggle_source_play();
            return;
        }
        if self.playing {
            self.halt_transport();
            self.status = "Paused.".into();
            return;
        }
        self.halt_source_transport();
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

    fn toggle_source_play(&mut self) {
        if self.source_playing {
            self.halt_source_transport();
            self.status = "Source paused.".into();
            return;
        }
        if self.selected_media.is_none() {
            self.status = "Select a clip in the media pool.".into();
            return;
        }
        self.halt_transport();
        self.source_play_rate = 1;
        self.source_playing = true;
        self.source_play_accum = 0.0;
        self.status = "Source play.".into();
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
                    collect_bus_pieces(
                        sequence,
                        &project.media,
                        &project.multicam_groups,
                        playhead,
                        end,
                    ),
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
        if self.focused_monitor == MonitorFocus::Source {
            self.step_source_playhead(delta);
            return;
        }
        self.halt_transport();
        self.preview_scrub = true;
        self.reveal_playhead = true;
        self.playhead = (self.playhead + delta).max(0);
    }

    fn step_source_playhead(&mut self, delta: i64) {
        self.halt_source_transport();
        self.preview_scrub = true;
        let Some(media_id) = self.selected_media else {
            return;
        };
        let end = self
            .session
            .project()
            .media(media_id)
            .map(|media| media.duration.0.max(0))
            .unwrap_or(0);
        let marks = self.source_marks.entry(media_id).or_default();
        marks.playhead = (marks.playhead + delta).clamp(0, end);
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
        } else if tap(Key::Slash) {
            self.split_at_playhead();
        } else if tap(Key::Q) && !mods.command {
            self.ripple_trim_prev_to_playhead();
        } else if tap(Key::W) && !mods.command {
            self.ripple_trim_next_to_playhead();
        } else if tap(Key::F) {
            self.zoom_to_fit();
            self.reveal_playhead = true;
        } else if tap(Key::Comma) {
            self.place_selected_media(false);
        } else if tap(Key::Period) {
            self.place_selected_media(true);
        } else if tap(Key::Backslash) {
            self.toggle_monitor_focus();
        } else if let Some(digit) = tap_digit(ctx, &mods) {
            if mods.shift {
                self.toggle_audio_target(digit);
            } else if !mods.command && !mods.alt {
                self.toggle_video_target(digit);
            }
        } else if let Some(angle) = angle_hotkey(ctx) {
            self.switch_multicam_angle(angle);
        } else if held_cmd(Key::ArrowLeft) {
            self.step_playhead(-second);
        } else if held_cmd(Key::ArrowRight) {
            self.step_playhead(second);
        } else if tap(Key::Space) {
            self.toggle_play();
        } else if tap(Key::J) {
            self.shuttle(-1);
        } else if tap(Key::K) {
            if self.focused_monitor == MonitorFocus::Source {
                self.halt_source_transport();
            } else {
                self.halt_transport();
            }
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
            if self.focused_monitor == MonitorFocus::Program {
                self.jump_edit(-1);
            }
        } else if pressed(Key::ArrowDown) {
            if self.focused_monitor == MonitorFocus::Program {
                self.jump_edit(1);
            }
        } else if pressed(Key::Home) {
            if self.focused_monitor == MonitorFocus::Source {
                self.halt_source_transport();
                self.set_source_playhead(0);
            } else {
                self.halt_transport();
                self.playhead = 0;
                self.reveal_playhead = true;
            }
        } else if pressed(Key::End) {
            if self.focused_monitor == MonitorFocus::Source {
                self.halt_source_transport();
                let end = self
                    .selected_media
                    .and_then(|id| self.session.project().media(id))
                    .map(|media| media.duration.0.max(0))
                    .unwrap_or(0);
                self.set_source_playhead(end);
            } else {
                self.halt_transport();
                self.playhead = self.sequence_end();
                self.reveal_playhead = true;
            }
        } else if pressed(Key::I) && !mods.command {
            self.mark_in();
        } else if pressed(Key::O) && !mods.command {
            self.mark_out();
        } else if tap(Key::M) {
            self.add_marker();
        } else if pressed(Key::OpenBracket) {
            if self.focused_monitor == MonitorFocus::Program {
                self.jump_marker(-1);
            }
        } else if pressed(Key::CloseBracket) {
            if self.focused_monitor == MonitorFocus::Program {
                self.jump_marker(1);
            }
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
            if self.selected.is_empty() && self.selected_marker.is_some() {
                self.delete_selected_marker();
            } else {
                self.delete_selection(false);
            }
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
            self.selected_marker = None;
            self.renaming_bin = None;
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
        if self.focused_monitor == MonitorFocus::Source {
            self.mark_source_in();
            return;
        }
        self.mark_program_in();
    }

    pub fn mark_out(&mut self) {
        if self.focused_monitor == MonitorFocus::Source {
            self.mark_source_out();
            return;
        }
        self.mark_program_out();
    }

    pub fn mark_program_in(&mut self) {
        let frame = Frame(self.playhead);
        if let Err(err) = self.session.set_in_point(Some(frame)) {
            self.status = err.to_string();
        } else {
            self.status = format!("In {}", format_tc(self.playhead, self.timebase()));
        }
    }

    pub fn mark_program_out(&mut self) {
        let frame = Frame(self.playhead);
        if let Err(err) = self.session.set_out_point(Some(frame)) {
            self.status = err.to_string();
        } else {
            self.status = format!("Out {}", format_tc(self.playhead, self.timebase()));
        }
    }

    pub fn clear_marks(&mut self) {
        if self.focused_monitor == MonitorFocus::Source {
            if let Some(media_id) = self.selected_media {
                let marks = self.source_marks.entry(media_id).or_default();
                marks.in_point = None;
                marks.out_point = None;
                self.status = "Cleared source in and out.".into();
            } else {
                self.status = "Select a clip in the media pool.".into();
            }
            return;
        }
        let _ = self.session.set_in_point(None);
        let _ = self.session.set_out_point(None);
        self.status = "Cleared in and out.".into();
    }

    fn mark_source_in(&mut self) {
        let Some(media_id) = self.selected_media else {
            self.status = "Select a clip in the media pool.".into();
            return;
        };
        let Some(media) = self.session.project().media(media_id).cloned() else {
            self.status = "Media not found.".into();
            return;
        };
        let end = media.duration.0.max(0);
        let marks = self.source_marks.entry(media_id).or_default();
        let frame = marks.playhead.clamp(0, end);
        // Keep at least one frame of room for out when marking near the end.
        marks.in_point = Some(frame.min(end.saturating_sub(1)));
        if marks
            .out_point
            .is_some_and(|out| out <= marks.in_point.unwrap_or(0))
        {
            marks.out_point = None;
        }
        self.status = format!(
            "Source in {}",
            format_tc(marks.in_point.unwrap_or(0), media.timebase)
        );
    }

    fn mark_source_out(&mut self) {
        let Some(media_id) = self.selected_media else {
            self.status = "Select a clip in the media pool.".into();
            return;
        };
        let Some(media) = self.session.project().media(media_id).cloned() else {
            self.status = "Media not found.".into();
            return;
        };
        let end = media.duration.0.max(0);
        let marks = self.source_marks.entry(media_id).or_default();
        let mut frame = marks.playhead.clamp(0, end);
        if let Some(inn) = marks.in_point {
            if frame <= inn {
                frame = (inn + 1).min(end);
            }
        } else if frame < 1 {
            frame = 1.min(end);
        }
        marks.out_point = Some(frame);
        self.status = format!(
            "Source out {}",
            format_tc(marks.out_point.unwrap_or(0), media.timebase)
        );
    }

    pub fn add_marker(&mut self) {
        let name = format!("Marker {}", format_tc(self.playhead, self.timebase()));
        match self.session.add_marker(Frame(self.playhead), &name) {
            Ok(()) => self.status = format!("Added {name}."),
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn delete_selected_marker(&mut self) {
        let Some(marker_id) = self.selected_marker else {
            self.status = "No marker selected.".into();
            return;
        };
        match self.session.delete_marker(marker_id) {
            Ok(()) => {
                self.selected_marker = None;
                self.status = "Deleted marker.".into();
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn jump_to_marker(&mut self, marker_id: MarkerId) {
        let marker_name = self
            .session
            .project()
            .active()
            .and_then(|sequence| {
                sequence
                    .markers
                    .iter()
                    .find(|marker| marker.id == marker_id)
                    .map(|marker| (marker.frame.0, marker.name.clone()))
            });
        if let Some((frame, name)) = marker_name {
            self.playhead = frame;
            self.selected_marker = Some(marker_id);
            self.halt_transport();
            self.reveal_playhead = true;
            self.status = format!("Marker — {name}.");
        }
    }

    pub fn jump_marker(&mut self, direction: i64) {
        let Some(sequence) = self.session.project().active() else {
            return;
        };
        let markers = &sequence.markers;
        if markers.is_empty() {
            self.status = "No markers in this sequence.".into();
            return;
        }
        let target = if direction > 0 {
            markers
                .iter()
                .find(|marker| marker.frame.0 > self.playhead)
                .or_else(|| markers.first())
        } else {
            markers
                .iter()
                .rev()
                .find(|marker| marker.frame.0 < self.playhead)
                .or_else(|| markers.last())
        };
        if let Some(marker) = target {
            self.jump_to_marker(marker.id);
        }
    }

    pub fn update_marker_name(&mut self, marker_id: MarkerId, name: String) {
        match self.session.update_marker(marker_id, Some(name), None, None, None) {
            Ok(()) => {}
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn update_marker_color(&mut self, marker_id: MarkerId, color: LabelColor) {
        match self.session.update_marker(marker_id, None, Some(color), None, None) {
            Ok(()) => {}
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn update_marker_comment(&mut self, marker_id: MarkerId, comment: String) {
        match self.session.update_marker(marker_id, None, None, Some(comment), None) {
            Ok(()) => {}
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn create_pool_bin(&mut self, parent: Option<BinId>) {
        let count = self.session.project().bins.len() + 1;
        let name = if count == 1 {
            "Master".to_string()
        } else {
            format!("Bin {count}")
        };
        match self.session.create_bin(parent, &name) {
            Ok(bin_id) => {
                self.selected_bin = Some(bin_id);
                self.renaming_bin = Some((bin_id, name.clone()));
                self.status = format!("Created {name}.");
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn rename_pool_bin(&mut self, bin_id: BinId, name: String) {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            self.status = "Bin name cannot be empty.".into();
            return;
        }
        match self.session.rename_bin(bin_id, trimmed) {
            Ok(()) => {
                self.renaming_bin = None;
                self.status = format!("Renamed bin to {trimmed}.");
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn delete_pool_bin(&mut self, bin_id: BinId) {
        match self.session.delete_bin(bin_id) {
            Ok(()) => {
                if self.selected_bin == Some(bin_id) {
                    self.selected_bin = self.session.project().bins.first().map(|bin| bin.id);
                }
                self.renaming_bin = None;
                self.status = "Deleted bin.".into();
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn move_pool_selection_to_bin(&mut self, bin_id: BinId) {
        if self.pool_selection.is_empty() {
            self.status = "Select media to move.".into();
            return;
        }
        let ids = self.pool_selection.clone();
        match self.session.move_media_to_bin(&ids, bin_id) {
            Ok(()) => {
                self.selected_bin = Some(bin_id);
                self.status = format!("Moved {} item(s) to bin.", ids.len());
            }
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

    pub fn add_title(&mut self) {
        let playhead = self.playhead.max(0);
        let created = std::cell::Cell::new(None);
        let result = self.session.edit("New Title", |project| {
            let sequence = project.active_sequence.ok_or(EditError::NoActiveSequence)?;
            let id = add_title(project, sequence, Frame(playhead), "Title")?;
            created.set(Some(id));
            Ok(())
        });
        self.status = match result {
            Ok(()) => {
                if let Some(id) = created.get() {
                    self.selected = vec![id];
                    self.selected_cue = None;
                }
                format!("Title at {}.", format_tc(playhead, self.timebase()))
            }
            Err(err) => err.to_string(),
        };
    }

    pub fn add_adjustment_layer(&mut self) {
        let playhead = self.playhead.max(0);
        let created = std::cell::Cell::new(None);
        let result = self.session.edit("New Adjustment Layer", |project| {
            let sequence = project.active_sequence.ok_or(EditError::NoActiveSequence)?;
            let id = add_adjustment_layer(project, sequence, Frame(playhead), "Adjustment")?;
            created.set(Some(id));
            Ok(())
        });
        self.status = match result {
            Ok(()) => {
                if let Some(id) = created.get() {
                    self.selected = vec![id];
                    self.selected_cue = None;
                }
                format!("Adjustment layer at {}.", format_tc(playhead, self.timebase()))
            }
            Err(err) => err.to_string(),
        };
    }

    pub fn create_multicam_from_pool(&mut self) {
        let ids = if self.pool_selection.len() >= 2 {
            self.pool_selection.clone()
        } else if let Some(id) = self.selected_media {
            vec![id]
        } else {
            self.status = "Shift-click at least two video clips in the pool.".into();
            return;
        };
        let playhead = self.playhead.max(0);
        let mut created = None;
        self.halt_transport();
        let result = self.session.edit("Create multicam", |project| {
            let sequence = project.active_sequence.ok_or(EditError::NoActiveSequence)?;
            created = Some(create_multicam(project, sequence, &ids, Frame(playhead))?);
            Ok(())
        });
        self.status = match result {
            Ok(()) => {
                if let Some(id) = created {
                    if let Some(sequence) = self.session.project().active() {
                        self.selected = if self.linked_selection {
                            expand_linked(sequence, &[id])
                        } else {
                            vec![id]
                        };
                    }
                }
                let (name, count) = self
                    .session
                    .project()
                    .multicam_groups
                    .last()
                    .map(|group| (group.name.clone(), group.angles.len()))
                    .unwrap_or_else(|| ("Multicam".into(), 0));
                format!(
                    "{name} · {count} angles at {}. Alt+1–9 or the angle bank switches.",
                    format_tc(playhead, self.timebase())
                )
            }
            Err(err) => err.to_string(),
        };
    }

    pub fn nest_selection(&mut self) {
        let ids = self.selected_with_links();
        if ids.is_empty() {
            self.status = "Select clips to nest.".into();
            return;
        }
        let mut created = None;
        self.halt_transport();
        let result = self.session.edit("Nest selection", |project| {
            let sequence = project.active_sequence.ok_or(EditError::NoActiveSequence)?;
            created = Some(create_nested_sequence(project, sequence, &ids)?);
            Ok(())
        });
        self.status = match result {
            Ok(()) => {
                if let Some(id) = created {
                    if let Some(sequence) = self.session.project().active() {
                        self.selected = if self.linked_selection {
                            expand_linked(sequence, &[id])
                        } else {
                            vec![id]
                        };
                    }
                }
                "Nested selection into a compound clip.".into()
            }
            Err(err) => err.to_string(),
        };
    }

    pub fn open_nested_sequence(&mut self, clip_id: ClipId) {
        let prepared = {
            let project = self.session.project();
            let Some(sequence) = project.active() else {
                return;
            };
            let Some(clip) = sequence.clip(clip_id) else {
                return;
            };
            let Some(child) = nested_sequence_id(clip) else {
                return;
            };
            (sequence.id, child, clip.name.clone())
        };
        let (parent, child, name) = prepared;
        self.halt_transport();
        self.session.edit("Open nested sequence", |project| {
            project.active_sequence = Some(child);
            Ok(())
        });
        self.sequence_nav_stack.push(parent);
        self.selected.clear();
        self.status = format!("Editing nested sequence “{name}”.");
    }

    pub fn close_nested_sequence(&mut self) {
        let Some(parent) = self.sequence_nav_stack.pop() else {
            return;
        };
        self.halt_transport();
        self.session.edit("Close nested sequence", |project| {
            project.active_sequence = Some(parent);
            Ok(())
        });
        self.selected.clear();
        if let Some(sequence) = self.session.project().active() {
            self.status = format!("Back to “{}”.", sequence.name);
        }
    }

    pub fn switch_multicam_angle(&mut self, angle: u32) {
        let playhead = self.playhead.max(0);
        let selected = self.selected.clone();
        let prepared = {
            let project = self.session.project();
            let Some(sequence) = project.active() else {
                return;
            };
            let Some(clip_id) = multicam_target(sequence, &selected, playhead) else {
                return;
            };
            let Some(binding) = sequence
                .clip(clip_id)
                .and_then(|clip| clip.multicam.clone())
            else {
                return;
            };
            let Some(group) = project.multicam_group(binding.group) else {
                return;
            };
            if angle as usize >= group.angles.len() {
                return;
            }
            let name = group.angles[angle as usize].name.clone();
            (sequence.id, clip_id, name)
        };
        let (sequence_id, clip_id, angle_name) = prepared;
        self.halt_transport();
        let mut switched = None;
        let result = self.session.edit("Switch angle", |project| {
            switched = Some(switch_angle(
                project,
                sequence_id,
                clip_id,
                Frame(playhead),
                angle,
            )?);
            Ok(())
        });
        match result {
            Ok(()) => {
                if let Some(id) = switched {
                    if let Some(sequence) = self.session.project().active() {
                        self.selected = if self.linked_selection {
                            expand_linked(sequence, &[id])
                        } else {
                            vec![id]
                        };
                    }
                }
                self.status = format!("Angle {} · {angle_name}.", angle + 1);
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn set_multicam_sync(&mut self, group: MulticamId, angle: u32, offset: i64) {
        let result = self.session.edit("Angle sync", |project| {
            set_angle_sync(project, group, angle, offset)
        });
        if let Err(err) = result {
            self.status = err.to_string();
        }
    }

    pub fn place_selected_media(&mut self, insert: bool) {
        let Some(media_id) = self.selected_media else {
            self.status = "Select a clip in the media pool.".into();
            return;
        };
        let Some(media) = self.session.project().media(media_id).cloned() else {
            self.status = "Media not found.".into();
            return;
        };
        let (source_in, source_out) = match self.source_edit_range(&media) {
            Ok(range) => range,
            Err(err) => {
                self.status = err.to_string();
                return;
            }
        };
        let playhead = self.playhead.max(0);
        let label = if insert { "Insert" } else { "Overwrite" };
        let targeted = self.targeted_tracks.clone();
        let result = self.session.edit(label, |project| {
            place_media(
                project,
                media_id,
                playhead,
                insert,
                &targeted,
                source_in,
                source_out,
            )
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
                let span = format_tc(source_out.0 - source_in.0, media.timebase);
                format!(
                    "{label} {span} at {}.",
                    format_tc(playhead, self.timebase())
                )
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
        let groups = self.session.project().multicam_groups.clone();
        let pieces = collect_pieces(sequence, &media, &groups, start, end);
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
        let snap = self.proxy_job.as_ref().map(|job| job.snapshot());
        if let Some(snap) = snap {
            self.proxy_note = snap.message.clone();
            if snap.finished {
                self.proxy_job = None;
                if !snap.attached.is_empty() {
                    let links = snap
                        .attached
                        .iter()
                        .map(|(id, path)| (MediaId(*id), path.clone()))
                        .collect();
                    match self.session.attach_proxies(links) {
                        Ok(()) => self.status = snap.message,
                        Err(err) => self.status = err.to_string(),
                    }
                } else {
                    self.status = snap.message;
                }
                self.proxy_note.clear();
            } else {
                self.status = snap.message;
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
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
        let frames = self.sequence_end().max(1) as f32;
        let width = self
            .timeline_view
            .map(|rect| rect.width())
            .unwrap_or(900.0)
            .max(80.0);
        self.pixels_per_frame = editor_core::clamp_timeline_zoom(width / frames);
        self.timeline_origin = 0.0;
    }

    pub fn toggle_proxies(&mut self) {
        let next = !self.session.project().prefer_proxies;
        self.session.set_prefer_proxies(next);
        self.status = if next {
            "Prefer proxies. Preview uses a proxy when that file is on disk, and the original otherwise.".into()
        } else {
            "Full resolution preview. Export always uses the original.".into()
        };
    }

    pub fn cancel_proxies(&mut self) {
        if let Some(job) = &self.proxy_job {
            job.cancel();
            self.proxy_note = "Cancelling proxy generation…".into();
            self.status = self.proxy_note.clone();
        }
    }

    pub fn generate_proxies(&mut self, whole_project: bool) {
        if self.proxy_job.is_some() {
            self.status = "A proxy job is already running.".into();
            return;
        }
        let project_path = self.path.clone();
        let dir = match project_path.as_deref() {
            Some(path) => editor_media::project_proxy_dir(std::path::Path::new(path)),
            None => editor_media::unsaved_proxy_dir(),
        };
        let selected = self.selected_media;
        let items: Vec<crate::proxy_job::ProxyItem> = self
            .session
            .project()
            .media
            .iter()
            .filter(|media| media.has_video)
            .filter(|media| whole_project || selected == Some(media.id))
            .filter(|media| !ui::media_missing(&media.path))
            .map(|media| {
                let source = resolve_media_path(&media.path);
                let output = editor_media::proxy_output_path(&dir, &source);
                crate::proxy_job::ProxyItem {
                    media_id: media.id.0,
                    name: media.name.clone(),
                    source,
                    output,
                }
            })
            .collect();
        if items.is_empty() {
            self.status = if whole_project {
                "No online video in the project to proxy.".into()
            } else {
                "Select an online video clip in the pool.".into()
            };
            return;
        }
        let total = items.len();
        self.proxy_note = format!("Proxy 0/{total}");
        self.status = self.proxy_note.clone();
        self.proxy_job = Some(crate::proxy_job::spawn_proxies(items));
    }

    pub fn relink_selected(&mut self) {
        let Some(id) = self.selected_media else {
            self.status = "Select a pool item to relink.".into();
            return;
        };
        self.relink_media(id);
    }

    pub fn relink_media(&mut self, id: MediaId) {
        let Some(path) = dialogs::relink_media_file() else {
            self.status = "Relink cancelled.".into();
            return;
        };
        let canonical = canonical_media_path(&path);
        let probed = match probe(std::path::Path::new(&canonical)) {
            Ok(result) => result,
            Err(err) => {
                self.status = err.to_string();
                return;
            }
        };
        let mut asset = asset_from_probe(&probed);
        asset.path = canonical.clone();
        asset.offline = ui::media_missing(&canonical);
        let name = self
            .session
            .project()
            .media(id)
            .map(|media| media.name.clone())
            .unwrap_or_else(|| "media".into());
        match self.session.relink_media(id, asset) {
            Ok(()) => {
                let state = if ui::media_missing(&canonical) {
                    "offline"
                } else {
                    "online"
                };
                self.status = format!("Relinked {name} ({state}) — {canonical}");
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    pub fn open_dense(&mut self) {
        self.session.replace_project(editor_core::dense_project());
        self.path = None;
        self.playhead = 0;
        self.selected.clear();
        self.selected_media = Some(MediaId(10));
        self.pool_selection = vec![MediaId(10)];
        self.pixels_per_frame = editor_core::clamp_timeline_zoom(0.05);
        self.timeline_origin = 0.0;
        self.halt_transport();
        self.workspace = Workspace::Edit;
        self.reset_track_targets();
        self.status = "Opened a 400-clip sequence. Fit shows minutes; zoom in to frames.".into();
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
        let target_bin = self.selected_bin;
        let result = self.session.edit("Import media", move |project| {
            ensure_master_bin(project);
            for mut asset in assets {
                if let Some(bin_id) = target_bin {
                    asset.bin_id = bin_id;
                }
                editor_core::import_media(project, asset);
            }
            Ok(())
        });
        match result {
            Ok(()) => {
                self.selected_media = self.session.project().media.last().map(|media| media.id);
                self.pool_selection = self.selected_media.into_iter().collect();
                let extra = if errors.is_empty() {
                    String::new()
                } else {
                    format!("  {}", errors.join("  "))
                };
                self.status = format!(
                    "Imported {count} file(s). Double-click to open in source, or use Overwrite / Insert.{extra}"
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
                        self.pool_selection = vec![id];
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
                self.pool_selection.clear();
                self.halt_transport();
                self.reset_track_targets();
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
                self.pool_selection.clear();
                self.halt_transport();
                self.workspace = Workspace::Edit;
                self.reset_track_targets();
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
        self.pool_selection = vec![MediaId(10)];
        self.halt_transport();
        self.workspace = Workspace::Edit;
        self.reset_track_targets();
        self.status = "Opened example project — Northline — Opening.".into();
    }

    pub(crate) fn deliver_presets(&self) -> Vec<editor_core::DeliverPreset> {
        editor_core::all_deliver_presets(Some(&editor_core::custom_deliver_preset_dir()))
    }

    pub(crate) fn apply_deliver_preset(&mut self, preset_id: &str) {
        let Some(preset) = self
            .deliver_presets()
            .into_iter()
            .find(|preset| preset.id == preset_id)
        else {
            self.deliver.report = format!("Unknown deliver preset “{preset_id}”.");
            return;
        };
        let sequence_name = self
            .session
            .project()
            .active()
            .map(|seq| seq.name.clone())
            .unwrap_or_else(|| "export".into());
        let settings = editor_core::DeliverSettings::from_preset(
            &preset,
            &sequence_name,
            &self.deliver.output_path,
        );
        self.deliver = DeliverState::from_settings(settings);
        self.deliver.report = format!("Applied preset “{}”.", preset.name);
    }

    pub(crate) fn save_custom_deliver_preset(&mut self) -> Result<(), String> {
        let name = self.deliver.save_preset_name.trim();
        if name.is_empty() {
            return Err("Enter a name for the custom preset.".into());
        }
        let id = name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_string();
        if id.is_empty() {
            return Err("Preset name must contain letters or numbers.".into());
        }
        let preset = editor_core::DeliverPreset {
            id: id.clone(),
            name: name.into(),
            description: if self.deliver.save_preset_note.trim().is_empty() {
                format!(
                    "Custom preset saved from {} / {}",
                    self.deliver.codec, self.deliver.container
                )
            } else {
                self.deliver.save_preset_note.trim().into()
            },
            codec: self.deliver.codec.clone(),
            container: self.deliver.container.clone(),
            filename_suffix: format!("_{id}"),
            burn_captions: self.deliver.burn_captions,
            use_in_out: self.deliver.use_in_out,
            video_bitrate_kbps: self.deliver.video_bitrate_kbps,
            audio_bitrate_kbps: self.deliver.audio_bitrate_kbps,
            audio_only: self.deliver.audio_only,
        };
        let path = editor_core::custom_deliver_preset_dir().join(format!("{id}.json"));
        editor_core::save_deliver_preset(&path, &preset).map_err(|err| err.to_string())?;
        self.deliver.preset_id = id;
        self.deliver.report = format!("Saved custom preset to {}.", path.display());
        Ok(())
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
        let groups = self.session.project().multicam_groups.clone();
        let range = if self.deliver.use_in_out {
            ExportRange::InOut
        } else {
            ExportRange::WholeSequence
        };
        let output = self.deliver.output_path.clone();
        let _ = editor_core::save_last_deliver_settings(&self.deliver.to_settings());
        #[cfg(feature = "ffmpeg")]
        {
            let sequences = self.session.project().sequences.clone();
            match editor_media::plan_encode_with(
                &sequence,
                &media,
                &groups,
                &sequences,
                range,
                &self.deliver.codec,
                &self.deliver.container,
                self.deliver.burn_captions,
                self.deliver.encode_hints(),
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
            let _ = (sequence, media, groups, range, output);
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
    targeted: &HashSet<TrackId>,
    source_in: Frame,
    source_out: Frame,
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
        .find(|t| {
            t.kind == TrackKind::Video && !t.locked && targeted.contains(&t.id)
        })
        .map(|t| t.id);
    let audio_track = project
        .sequence(seq_id)
        .unwrap()
        .tracks
        .iter()
        .find(|t| {
            t.kind == TrackKind::Audio && !t.locked && targeted.contains(&t.id)
        })
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
                source_in,
                source_out,
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
                source_in,
                source_out,
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
        proxy_path: None,
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

fn tap_digit(ctx: &egui::Context, mods: &Modifiers) -> Option<u32> {
    if mods.command || mods.alt {
        return None;
    }
    let key = [
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
        Key::Num7,
        Key::Num8,
        Key::Num9,
    ]
    .into_iter()
    .enumerate()
    .find_map(|(index, key)| consume_key(ctx, Modifiers::NONE, key, false).then_some(index as u32 + 1));
    key
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
                    .show(ctx, |ui| ui::dual_monitor_panel(ui, self));
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
                        if ui.button("Relink Media…").clicked() {
                            self.relink_selected();
                            ui.close_menu();
                        }
                        if ui.button("New Title").clicked() {
                            self.add_title();
                            ui.close_menu();
                        }
                        if ui.button("New Adjustment Layer").clicked() {
                            self.add_adjustment_layer();
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Generate Proxies for Selection").clicked() {
                            self.generate_proxies(false);
                            ui.close_menu();
                        }
                        if ui.button("Generate Proxies for Project").clicked() {
                            self.generate_proxies(true);
                            ui.close_menu();
                        }
                        let proxy_label = if self.session.project().prefer_proxies {
                            "Use Full Resolution"
                        } else {
                            "Prefer Proxies"
                        };
                        if ui.button(proxy_label).clicked() {
                            self.toggle_proxies();
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
                        if ui.button("Create Multicam from Pool").clicked() {
                            self.create_multicam_from_pool();
                            ui.close_menu();
                        }
                        if ui.button("Nest Selection").clicked() {
                            self.nest_selection();
                            ui.close_menu();
                        }
                    });
                    ui.menu_button("Sequence", |ui| {
                        if ui
                            .add_enabled(
                                !self.sequence_nav_stack.is_empty(),
                                egui::Button::new("Close Nested Sequence"),
                            )
                            .clicked()
                        {
                            self.close_nested_sequence();
                            ui.close_menu();
                        }
                        if ui.button("Open Nested Sequence").clicked() {
                            if let Some(id) = self.selected.first().copied() {
                                self.open_nested_sequence(id);
                            } else {
                                self.status = "Select a nested clip to open.".into();
                            }
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Dense Sequence (400 clips)").clicked() {
                            self.open_dense();
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Mark In").clicked() {
                            self.mark_program_in();
                            ui.close_menu();
                        }
                        if ui.button("Mark Out").clicked() {
                            self.mark_program_out();
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
                        if ui.button("Dip to Black at Cut").clicked() {
                            self.add_transition_at_selection(TransitionKind::DipToBlack);
                            ui.close_menu();
                        }
                        if ui.button("Dip to White at Cut").clicked() {
                            self.add_transition_at_selection(TransitionKind::DipToWhite);
                            ui.close_menu();
                        }
                        if ui.button("Slide Left at Cut").clicked() {
                            self.add_transition_at_selection(TransitionKind::Slide {
                                direction: Direction::Left,
                            });
                            ui.close_menu();
                        }
                        if ui.button("Slide Right at Cut").clicked() {
                            self.add_transition_at_selection(TransitionKind::Slide {
                                direction: Direction::Right,
                            });
                            ui.close_menu();
                        }
                        if ui.button("Blur Dissolve at Cut").clicked() {
                            self.add_transition_at_selection(TransitionKind::BlurDissolve);
                            ui.close_menu();
                        }
                        if ui.button("Iris at Cut").clicked() {
                            self.add_transition_at_selection(TransitionKind::Iris);
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
                            RichText::new(editor_core::timeline_scale_label(
                                self.pixels_per_frame,
                                self.timebase().fps_f64(),
                            ))
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
    ("I / O", "Mark in / out on the focused monitor (source or program)"),
    ("\\", "Toggle focus between source and program monitors"),
    ("M", "Add marker"),
    ("[ / ]", "Previous / next marker"),
    ("V", "Select tool"),
    ("C  /  Ctrl+K", "Razor at the playhead (also selects the razor tool)"),
    ("/", "Razor at the playhead (keeps the active tool)"),
    ("Q / W", "Ripple trim previous / next edit to the playhead (targeted tracks)"),
    (", / .", "Overwrite / insert source in–out at the program playhead"),
    ("1–9", "Toggle video track target (V1–V9)"),
    ("Shift+1–9", "Toggle audio track target (A1–A9)"),
    ("Alt+1–9", "Switch multicam angle at the playhead"),
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
    ("F  /  Shift+Z", "Fit sequence in the timeline"),
    ("Ctrl+scroll  /  Alt+scroll  /  pinch", "Zoom timeline"),
    ("Middle-drag (vertical)", "Zoom timeline"),
    ("Scroll", "Pan timeline"),
    ("Double-click empty timeline", "Play / pause"),
];

fn angle_hotkey(ctx: &egui::Context) -> Option<u32> {
    const KEYS: [Key; 9] = [
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
        Key::Num7,
        Key::Num8,
        Key::Num9,
    ];
    for (index, key) in KEYS.into_iter().enumerate() {
        if consume_key(ctx, Modifiers::ALT, key, false) {
            return Some(index as u32);
        }
    }
    None
}

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
