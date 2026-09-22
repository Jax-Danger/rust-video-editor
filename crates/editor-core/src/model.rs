//! Project, media pool, sequences, tracks, clips, and markers.

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::effects::{AnimatedF32, Effect};
use crate::compressor::TrackCompressor;
use crate::eq::TrackEq3;
use crate::time::{Frame, Timebase};

fn default_version() -> u32 {
    1
}
fn default_true() -> bool {
    true
}
fn default_volume() -> AnimatedF32 {
    AnimatedF32::constant(1.0)
}
fn is_unity_volume(value: &AnimatedF32) -> bool {
    value.keys.is_empty() && (value.base - 1.0).abs() < 1.0e-5
}
fn default_fader() -> f32 {
    1.0
}
fn is_unity_fader(value: &f32) -> bool {
    (*value - 1.0).abs() < 1.0e-5
}
fn is_center_pan(value: &f32) -> bool {
    value.abs() < 1.0e-5
}

fn serialize_volume<S: Serializer>(value: &AnimatedF32, serializer: S) -> Result<S::Ok, S::Error> {
    if value.keys.is_empty() {
        serializer.serialize_f32(value.base)
    } else {
        value.serialize(serializer)
    }
}

fn deserialize_volume<'de, D: Deserializer<'de>>(deserializer: D) -> Result<AnimatedF32, D::Error> {
    struct VolumeVisitor;
    impl<'de> Visitor<'de> for VolumeVisitor {
        type Value = AnimatedF32;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a linear gain or an animated gain")
        }

        fn visit_f32<E: de::Error>(self, value: f32) -> Result<Self::Value, E> {
            Ok(AnimatedF32::constant(value))
        }

        fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
            Ok(AnimatedF32::constant(value as f32))
        }

        fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
            Ok(AnimatedF32::constant(value as f32))
        }

        fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
            Ok(AnimatedF32::constant(value as f32))
        }

        fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
            AnimatedF32::deserialize(de::value::MapAccessDeserializer::new(map))
        }
    }
    deserializer.deserialize_any(VolumeVisitor)
}
fn default_label() -> LabelColor {
    LabelColor::Neutral
}

macro_rules! id_newtype {
    ($($name:ident),* $(,)?) => {
        $(
            #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
            pub struct $name(pub u64);

            impl $name {
                pub const fn new(value: u64) -> Self {
                    Self(value)
                }
            }

            impl std::fmt::Display for $name {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    write!(f, "{}", self.0)
                }
            }
        )*
    };
}

id_newtype!(
    BinId,
    MediaId,
    SequenceId,
    TrackId,
    ClipId,
    MarkerId,
    CueId,
    TransitionId,
    MulticamId,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Video,
    Audio,
    Caption,
}

impl TrackKind {
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Video => "V",
            Self::Audio => "A",
            Self::Caption => "C",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelColor {
    Neutral,
    Rose,
    Amber,
    Green,
    Teal,
    Blue,
    Violet,
}

impl Default for LabelColor {
    fn default() -> Self {
        Self::Neutral
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionAlign {
    Center,
    StartOnCut,
    EndOnCut,
}

impl Default for TransitionAlign {
    fn default() -> Self {
        Self::Center
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TransitionKind {
    CrossDissolve,
    Wipe {
        angle_deg: f32,
    },
    PushSlide {
        direction: Direction,
    },
    /// Outgoing fades to black, then incoming fades from black.
    DipToBlack,
    /// Outgoing fades to white, then incoming fades from white.
    DipToWhite,
    /// Outgoing stays put; incoming slides over it.
    Slide {
        direction: Direction,
    },
    /// Cross dissolve with a blur peak at the midpoint.
    BlurDissolve,
    /// Circular iris open/close centred on the frame.
    Iris,
}

impl TransitionKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::CrossDissolve => "Cross dissolve",
            Self::Wipe { .. } => "Wipe",
            Self::PushSlide { .. } => "Push",
            Self::DipToBlack => "Dip to black",
            Self::DipToWhite => "Dip to white",
            Self::Slide { .. } => "Slide",
            Self::BlurDissolve => "Blur dissolve",
            Self::Iris => "Iris",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub id: TransitionId,
    pub kind: TransitionKind,
    pub left_clip: ClipId,
    pub right_clip: ClipId,
    /// Duration in sequence frames.
    pub duration: i64,
    #[serde(default)]
    pub alignment: TransitionAlign,
}

impl Transition {
    /// Inclusive start, exclusive end, in sequence frames.
    pub fn range(&self, cut: Frame) -> (Frame, Frame) {
        let duration = self.duration.max(0);
        let (start, _end_offset) = match self.alignment {
            TransitionAlign::Center => (cut.0 - duration / 2, duration),
            TransitionAlign::StartOnCut => (cut.0, duration),
            TransitionAlign::EndOnCut => (cut.0 - duration, duration),
        };
        (Frame(start), Frame(start + duration))
    }

    pub fn progress(&self, cut: Frame, playhead: Frame) -> Option<f32> {
        let (start, end) = self.range(cut);
        if playhead.0 < start.0 || playhead.0 >= end.0 || end.0 <= start.0 {
            return None;
        }
        Some((playhead.0 - start.0) as f32 / (end.0 - start.0) as f32)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptionCue {
    pub id: CueId,
    pub timeline_in: Frame,
    pub timeline_out: Frame,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

/// Horizontal anchor for a title block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

impl Default for TextAlign {
    fn default() -> Self {
        Self::Center
    }
}

fn default_title_text() -> String {
    "Title".into()
}
fn default_title_size() -> f32 {
    0.064
}
fn default_title_color() -> [f32; 4] {
    [1.0, 1.0, 1.0, 1.0]
}
fn default_title_x() -> f32 {
    0.5
}
fn default_title_y() -> f32 {
    0.8
}
fn default_title_plate() -> f32 {
    0.55
}

/// One camera inside a [`MulticamGroup`].
///
/// `sync_offset` is the source frame on this angle that lines up with group
/// time 0. A camera that started recording earlier has a positive offset so
/// its clap matches the other angles. Optional `audio` replaces the video
/// file's own sound for this angle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MulticamAngle {
    pub name: String,
    pub video: MediaId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<MediaId>,
    #[serde(default)]
    pub sync_offset: Frame,
}

/// Angles that share one sync origin. Clips on the timeline point at the group
/// and store which angle is active over group time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MulticamGroup {
    pub id: MulticamId,
    pub name: String,
    /// Group time is measured on this timebase. Timeline clips store their
    /// source in and out in these frames.
    pub timebase: Timebase,
    pub angles: Vec<MulticamAngle>,
}

/// Angle change at a group-time frame. `at` uses the same units as the clip's
/// source in/out, so moving the clip on the timeline does not move the cut.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AngleCut {
    pub at: Frame,
    pub angle: u32,
}

/// Link from a timeline clip back to its [`MulticamGroup`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MulticamBinding {
    pub group: MulticamId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cuts: Vec<AngleCut>,
}

/// Link from a parent-timeline clip to a child [`Sequence`].
///
/// The clip's `source_in` / `source_out` are frames inside the child sequence.
/// Preview and export composite the child at that frame and paint the result
/// as one layer on the parent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NestedBinding {
    pub sequence: SequenceId,
}

/// Generator text drawn into the frame by the shared compositor.
///
/// `x` and `y` are normalized anchors (0…1, y down). `font_size` is the glyph
/// box as a fraction of the sequence height. `plate` is the opacity of the
/// bar behind the text; 0 draws glyphs only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Title {
    #[serde(default = "default_title_text")]
    pub text: String,
    #[serde(default = "default_title_size")]
    pub font_size: f32,
    #[serde(default = "default_title_color")]
    pub color: [f32; 4],
    #[serde(default)]
    pub align: TextAlign,
    #[serde(default = "default_title_x")]
    pub x: f32,
    #[serde(default = "default_title_y")]
    pub y: f32,
    #[serde(default = "default_title_plate")]
    pub plate: f32,
}

impl Title {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            font_size: default_title_size(),
            color: default_title_color(),
            align: TextAlign::Center,
            x: default_title_x(),
            y: default_title_y(),
            plate: default_title_plate(),
        }
    }

    /// Lower-third defaults: centered, near the bottom, with a readable plate.
    pub fn lower_third(text: impl Into<String>) -> Self {
        let mut title = Self::new(text);
        title.y = 0.82;
        title.font_size = 0.062;
        title.plate = 0.62;
        title
    }

    pub fn sanitized(mut self) -> Self {
        if self.text.trim().is_empty() {
            self.text.clear();
        }
        self.font_size = self.font_size.clamp(0.02, 0.2);
        self.x = self.x.clamp(0.0, 1.0);
        self.y = self.y.clamp(0.0, 1.0);
        self.plate = self.plate.clamp(0.0, 1.0);
        self.color = self.color.map(|channel| channel.clamp(0.0, 1.0));
        self
    }

    /// Timeline label: the first non-empty line, trimmed to a clip name.
    pub fn timeline_name(&self) -> String {
        let line = self
            .text
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("Title");
        let mut name: String = line.trim().chars().take(48).collect();
        if name.is_empty() {
            name = "Title".into();
        }
        name
    }
}

/// Inclusive playback-rate range. 1.0 is realtime. 0.25 is 25%, 4.0 is 400%.
pub const SPEED_MIN: f32 = 0.25;
pub const SPEED_MAX: f32 = 4.0;

pub fn clamp_speed(value: f32) -> f32 {
    if !value.is_finite() {
        return 1.0;
    }
    value.clamp(SPEED_MIN, SPEED_MAX)
}

fn unity_rate() -> AnimatedF32 {
    AnimatedF32::constant(1.0)
}

/// How fast source time moves relative to the sequence.
///
/// `rate` is an [`AnimatedF32`] in clip-relative sequence frames. No keys means
/// a constant `rate.base`. A linear ramp is a key at frame 0 and a key at frame
/// `-1`. The negative frame means the clip's current out point, so the ramp
/// stays stretched across the clip when its duration changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipSpeed {
    #[serde(default, skip_serializing_if = "is_false")]
    pub reverse: bool,
    #[serde(default = "unity_rate")]
    pub rate: AnimatedF32,
}

impl Default for ClipSpeed {
    fn default() -> Self {
        Self::normal()
    }
}

impl ClipSpeed {
    pub fn normal() -> Self {
        Self {
            reverse: false,
            rate: unity_rate(),
        }
    }

    pub fn is_identity(&self) -> bool {
        !self.reverse && self.rate.keys.is_empty() && (self.rate.base - 1.0).abs() < 1.0e-4
    }

    pub fn constant(rate: f32, reverse: bool) -> Self {
        Self {
            reverse,
            rate: AnimatedF32::constant(clamp_speed(rate)),
        }
    }

    /// Linear ramp from `start` at the first frame to `end` at the out point.
    pub fn ramp(start: f32, end: f32, reverse: bool) -> Self {
        let start = clamp_speed(start);
        let end = clamp_speed(end);
        let mut rate = AnimatedF32::constant(start);
        rate.set_key(0, start);
        rate.set_key(-1, end);
        Self { reverse, rate }
    }

    pub fn sanitized(mut self) -> Self {
        self.rate.base = clamp_speed(self.rate.base);
        for key in &mut self.rate.keys {
            key.value = clamp_speed(key.value);
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_id: Option<MediaId>,
    pub name: String,
    /// Inclusive sequence frame.
    pub timeline_in: Frame,
    /// Exclusive sequence frame.
    pub timeline_out: Frame,
    /// Inclusive source frame in [`Self::media_timebase`].
    pub source_in: Frame,
    /// Exclusive source frame in [`Self::media_timebase`].
    pub source_out: Frame,
    /// Inclusive media limit (usually 0).
    #[serde(default)]
    pub source_min: Frame,
    /// Exclusive media limit (media duration in source frames).
    pub source_max: Frame,
    #[serde(default)]
    pub media_timebase: Timebase,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linked: Vec<ClipId>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
    #[serde(default = "default_label")]
    pub label: LabelColor,
    /// Linear clip gain. 1 is unity. A bare number in JSON is a constant;
    /// an object is an [`AnimatedF32`] curve. Playback and export both read it.
    #[serde(
        default = "default_volume",
        skip_serializing_if = "is_unity_volume",
        serialize_with = "serialize_volume",
        deserialize_with = "deserialize_volume"
    )]
    pub volume: AnimatedF32,
    /// When set, this clip is a title generator. It has no media file; the
    /// compositor rasterizes [`Title`] into the frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<Title>,
    /// When true, this clip is an adjustment layer. It has no pixels of its
    /// own; grade and filters apply to the composite below during preview and
    /// export.
    #[serde(default, skip_serializing_if = "is_false")]
    pub adjustment: bool,
    /// Playback rate. Omitted from JSON at 100% forward. Preview and export
    /// both read it through [`crate::source_frame_at`].
    #[serde(default, skip_serializing_if = "ClipSpeed::is_identity")]
    pub speed: ClipSpeed,
    /// When set, preview and export play the active angle of this group
    /// instead of [`Self::media_id`] alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multicam: Option<MulticamBinding>,
    /// When set, preview and export rasterize this child sequence instead of
    /// decoding [`Self::media_id`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nested: Option<NestedBinding>,
}

impl Clip {
    pub fn duration(&self) -> i64 {
        self.timeline_out.0 - self.timeline_in.0
    }

    pub fn source_duration(&self) -> i64 {
        self.source_out.0 - self.source_in.0
    }

    pub fn head_handle(&self) -> i64 {
        self.source_in.0 - self.source_min.0
    }

    pub fn tail_handle(&self) -> i64 {
        self.source_max.0 - self.source_out.0
    }

    pub fn contains_frame(&self, frame: Frame) -> bool {
        frame.0 > self.timeline_in.0 && frame.0 < self.timeline_out.0
    }

    pub fn covers(&self, frame: Frame) -> bool {
        frame.0 >= self.timeline_in.0 && frame.0 < self.timeline_out.0
    }

    /// Test/helper constructor. Source starts at 0 with no handles.
    pub fn basic(id: u64, start: i64, end: i64) -> Self {
        let dur = end - start;
        Self {
            id: ClipId(id),
            media_id: None,
            name: format!("Clip {id}"),
            timeline_in: Frame(start),
            timeline_out: Frame(end),
            source_in: Frame(0),
            source_out: Frame(dur),
            source_min: Frame(0),
            source_max: Frame(dur),
            media_timebase: Timebase::fps_24(),
            linked: Vec::new(),
            enabled: true,
            effects: Vec::new(),
            label: LabelColor::Neutral,
            volume: AnimatedF32::constant(1.0),
            title: None,
            adjustment: false,
            speed: ClipSpeed::normal(),
            multicam: None,
            nested: None,
        }
    }

    /// A title generator. Source handles are an hour on each side so the clip
    /// can be trimmed longer without a media file.
    pub fn generator(id: u64, start: i64, end: i64, timebase: Timebase, title: Title) -> Self {
        let title = title.sanitized();
        let dur = (end - start).max(1);
        let pad = 24 * 60 * 60;
        Self {
            id: ClipId(id),
            media_id: None,
            name: title.timeline_name(),
            timeline_in: Frame(start),
            timeline_out: Frame(start + dur),
            source_in: Frame(pad),
            source_out: Frame(pad + dur),
            source_min: Frame(0),
            source_max: Frame(pad + dur + pad),
            media_timebase: timebase,
            linked: Vec::new(),
            enabled: true,
            effects: Vec::new(),
            label: LabelColor::Rose,
            volume: AnimatedF32::constant(1.0),
            title: Some(title),
            adjustment: false,
            speed: ClipSpeed::normal(),
            multicam: None,
            nested: None,
        }
    }

    /// An adjustment layer. Source handles are padded like a title so the clip
    /// can be trimmed longer without media.
    pub fn adjustment_layer(
        id: u64,
        start: i64,
        end: i64,
        timebase: Timebase,
        name: &str,
    ) -> Self {
        let dur = (end - start).max(1);
        let pad = 24 * 60 * 60;
        Self {
            id: ClipId(id),
            media_id: None,
            name: name.to_string(),
            timeline_in: Frame(start),
            timeline_out: Frame(start + dur),
            source_in: Frame(pad),
            source_out: Frame(pad + dur),
            source_min: Frame(0),
            source_max: Frame(pad + dur + pad),
            media_timebase: timebase,
            linked: Vec::new(),
            enabled: true,
            effects: Vec::new(),
            label: LabelColor::Amber,
            volume: AnimatedF32::constant(1.0),
            title: None,
            adjustment: true,
            speed: ClipSpeed::normal(),
            multicam: None,
            nested: None,
        }
    }

    pub fn is_title(&self) -> bool {
        self.title.is_some()
    }

    pub fn is_adjustment(&self) -> bool {
        self.adjustment
    }

    pub fn is_nested(&self) -> bool {
        self.nested.is_some()
    }

    /// Give the clip unused media before (`head`) and after (`tail`) the current source.
    pub fn with_handles(mut self, head: i64, tail: i64) -> Self {
        let dur = self.source_duration();
        self.source_in = Frame(head);
        self.source_out = Frame(head + dur);
        self.source_min = Frame(0);
        self.source_max = Frame(head + dur + tail);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub id: MarkerId,
    pub frame: Frame,
    #[serde(default)]
    pub duration: i64,
    pub name: String,
    #[serde(default)]
    pub color: LabelColor,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    pub name: String,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub solo: bool,
    /// When set, insert and ripple edits on other sync-locked tracks move this track too.
    #[serde(default = "default_true")]
    pub sync_lock: bool,
    /// Linear track fader. 1 is unity. Multiplied by clip gain in the mix.
    #[serde(default = "default_fader", skip_serializing_if = "is_unity_fader")]
    pub fader: f32,
    /// Stereo pan, −1 hard left, 0 center, +1 hard right.
    #[serde(default, skip_serializing_if = "is_center_pan")]
    pub pan: f32,
    /// Three-band EQ applied pre-fader on this track.
    #[serde(default, skip_serializing_if = "TrackEq3::is_bypass")]
    pub eq: TrackEq3,
    /// Dynamics compressor applied after EQ and before the fader.
    #[serde(default, skip_serializing_if = "TrackCompressor::is_bypass")]
    pub compressor: TrackCompressor,
    #[serde(default)]
    pub clips: Vec<Clip>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<Transition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cues: Vec<CaptionCue>,
    /// Indices of clips that end after a later clip. Empty on a normal cut.
    /// Rebuilt by [`Self::reindex`]. Not saved; the list is derived from the clips.
    #[serde(skip)]
    pub stacked_clips: Vec<u32>,
}

impl Track {
    pub fn new(id: TrackId, kind: TrackKind, name: impl Into<String>) -> Self {
        Self {
            id,
            kind,
            name: name.into(),
            locked: false,
            muted: false,
            solo: false,
            sync_lock: true,
            fader: 1.0,
            pan: 0.0,
            eq: TrackEq3::default(),
            compressor: TrackCompressor::default(),
            clips: Vec::new(),
            transitions: Vec::new(),
            cues: Vec::new(),
            stacked_clips: Vec::new(),
        }
    }

    /// Sort clips and cues, then record which clips run underneath later ones.
    pub fn reindex(&mut self) {
        self.clips.sort_by_key(|c| (c.timeline_in.0, c.id.0));
        self.cues.sort_by_key(|c| (c.timeline_in.0, c.id.0));
        self.stacked_clips = crate::scale::stacked_clip_indices(&self.clips);
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sequence {
    pub id: SequenceId,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub timebase: Timebase,
    #[serde(default = "default_pixel")]
    pub pixel_aspect_num: u32,
    #[serde(default = "default_pixel")]
    pub pixel_aspect_den: u32,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_point: Option<Frame>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_point: Option<Frame>,
    /// Linear master fader. 1 is unity. Applied after the track sum.
    #[serde(default = "default_fader", skip_serializing_if = "is_unity_fader")]
    pub master_fader: f32,
}

fn default_pixel() -> u32 {
    1
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl Sequence {
    pub fn new(
        id: SequenceId,
        name: impl Into<String>,
        width: u32,
        height: u32,
        timebase: Timebase,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            width,
            height,
            timebase,
            pixel_aspect_num: 1,
            pixel_aspect_den: 1,
            tracks: Vec::new(),
            markers: Vec::new(),
            in_point: None,
            out_point: None,
            master_fader: 1.0,
        }
    }

    pub fn add_track(&mut self, id: TrackId, kind: TrackKind, name: impl Into<String>) -> TrackId {
        self.tracks.push(Track::new(id, kind, name));
        id
    }

    pub fn end_frame(&self) -> Frame {
        let mut end = 0;
        for track in &self.tracks {
            for clip in &track.clips {
                end = end.max(clip.timeline_out.0);
            }
            for cue in &track.cues {
                end = end.max(cue.timeline_out.0);
            }
        }
        Frame(end)
    }

    pub fn locate_clip(&self, id: ClipId) -> Option<(usize, usize)> {
        for (ti, track) in self.tracks.iter().enumerate() {
            if let Some(ci) = track.clips.iter().position(|c| c.id == id) {
                return Some((ti, ci));
            }
        }
        None
    }

    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.locate_clip(id)
            .map(|(ti, ci)| &self.tracks[ti].clips[ci])
    }

    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn track_index(&self, id: TrackId) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }

    /// Video tracks drawn top-to-bottom (highest index first), then audio, then captions.
    pub fn visual_track_indices(&self) -> Vec<usize> {
        let mut video: Vec<usize> = self
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.kind == TrackKind::Video)
            .map(|(i, _)| i)
            .collect();
        video.reverse();
        let audio: Vec<usize> = self
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.kind == TrackKind::Audio)
            .map(|(i, _)| i)
            .collect();
        let captions: Vec<usize> = self
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.kind == TrackKind::Caption)
            .map(|(i, _)| i)
            .collect();
        video.into_iter().chain(audio).chain(captions).collect()
    }

    pub fn edit_points(&self) -> Vec<i64> {
        let mut points = vec![0];
        for track in &self.tracks {
            for clip in &track.clips {
                points.push(clip.timeline_in.0);
                points.push(clip.timeline_out.0);
            }
        }
        for marker in &self.markers {
            points.push(marker.frame.0);
        }
        points.sort_unstable();
        points.dedup();
        points
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bin {
    pub id: BinId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<BinId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaAsset {
    pub id: MediaId,
    pub bin_id: BinId,
    pub name: String,
    pub path: String,
    /// Duration in [`Self::timebase`] frames.
    pub duration: Frame,
    pub timebase: Timebase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_codec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_channels: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub has_video: bool,
    #[serde(default)]
    pub has_audio: bool,
    #[serde(default)]
    pub offline: bool,
    /// Lower-resolution H.264 preview. Absent until a proxy is generated.
    /// Preview falls back to [`Self::path`] when this file is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_path: Option<String>,
}

impl MediaAsset {
    pub fn kind_label(&self) -> &'static str {
        match (self.has_video, self.has_audio) {
            (true, true) => "A/V",
            (true, false) => "Video",
            (false, true) => "Audio",
            (false, false) => "File",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    #[serde(default = "default_version")]
    pub format_version: u32,
    pub name: String,
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub bins: Vec<Bin>,
    #[serde(default)]
    pub media: Vec<MediaAsset>,
    #[serde(default)]
    pub sequences: Vec<Sequence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_sequence: Option<SequenceId>,
    /// Program monitor prefers proxy media when the proxy file is on disk.
    /// Export ignores this and always uses the original.
    #[serde(default, skip_serializing_if = "is_false")]
    pub prefer_proxies: bool,
    /// Multicam groups shared by clips in any sequence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub multicam_groups: Vec<MulticamGroup>,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            format_version: 1,
            name: name.into(),
            next_id: 1,
            bins: Vec::new(),
            media: Vec::new(),
            sequences: Vec::new(),
            active_sequence: None,
            prefer_proxies: false,
            multicam_groups: Vec::new(),
        }
    }

    pub fn alloc(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    pub fn sequence(&self, id: SequenceId) -> Option<&Sequence> {
        self.sequences.iter().find(|s| s.id == id)
    }

    pub fn sequence_mut(&mut self, id: SequenceId) -> Option<&mut Sequence> {
        self.sequences.iter_mut().find(|s| s.id == id)
    }

    pub fn active(&self) -> Option<&Sequence> {
        self.active_sequence.and_then(|id| self.sequence(id))
    }

    pub fn active_mut(&mut self) -> Option<&mut Sequence> {
        let id = self.active_sequence?;
        self.sequence_mut(id)
    }

    pub fn media(&self, id: MediaId) -> Option<&MediaAsset> {
        self.media.iter().find(|m| m.id == id)
    }

    pub fn multicam_group(&self, id: MulticamId) -> Option<&MulticamGroup> {
        self.multicam_groups.iter().find(|group| group.id == id)
    }

    pub fn multicam_group_mut(&mut self, id: MulticamId) -> Option<&mut MulticamGroup> {
        self.multicam_groups.iter_mut().find(|group| group.id == id)
    }

    pub fn max_assigned_id(&self) -> u64 {
        let mut max_id = 0u64;
        let bump = |max_id: &mut u64, value: u64| {
            if value > *max_id {
                *max_id = value;
            }
        };
        for bin in &self.bins {
            bump(&mut max_id, bin.id.0);
        }
        for group in &self.multicam_groups {
            bump(&mut max_id, group.id.0);
        }
        for media in &self.media {
            bump(&mut max_id, media.id.0);
        }
        for seq in &self.sequences {
            bump(&mut max_id, seq.id.0);
            for marker in &seq.markers {
                bump(&mut max_id, marker.id.0);
            }
            for track in &seq.tracks {
                bump(&mut max_id, track.id.0);
                for clip in &track.clips {
                    bump(&mut max_id, clip.id.0);
                }
                for cue in &track.cues {
                    bump(&mut max_id, cue.id.0);
                }
                for transition in &track.transitions {
                    bump(&mut max_id, transition.id.0);
                }
            }
        }
        max_id
    }

    pub fn normalize(&mut self) {
        let max_id = self.max_assigned_id();
        if self.next_id <= max_id {
            self.next_id = max_id.saturating_add(1);
        }
        if self.active_sequence.is_none() {
            self.active_sequence = self.sequences.first().map(|s| s.id);
        }
        for seq in &mut self.sequences {
            for track in &mut seq.tracks {
                track.reindex();
            }
        }
    }

    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        let mut project: Project = serde_json::from_str(text)?;
        project.normalize();
        Ok(project)
    }

    pub fn save_file(&self, path: &std::path::Path) -> Result<(), ProjectIoError> {
        let json = self.to_json_pretty()?;
        std::fs::write(path, json + "\n")?;
        Ok(())
    }

    pub fn load_file(path: &std::path::Path) -> Result<Self, ProjectIoError> {
        let text = std::fs::read_to_string(path)?;
        Ok(Self::from_json(&text)?)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectIoError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
}
