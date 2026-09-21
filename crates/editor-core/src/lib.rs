//! Frame-accurate editorial core for the Meridian NLE.
//!
//! Time is an integer [`Frame`] on a rational [`Timebase`]. Floating-point
//! seconds are a display and pacing convenience, never the stored edit.

pub mod caption;
pub mod deliver;
pub mod demo;
pub mod edit;
pub mod effects;
pub mod model;
pub mod session;
pub mod template;
pub mod time;

pub use caption::{
    CaptionDraft, CaptionError, CaptionTranscriber, StubTranscriber, TranscribeRequest,
};
pub use deliver::{plan_export, ExportPlan, ExportRange};
pub use demo::demo_project;
pub use edit::{
    active_sequence_id, add_marker, add_transition, clip_from_media, collect_snap_points,
    delete_cue, expand_linked, import_media, insert_clips, lift_delete, link_clips, move_clips,
    overwrite_clips, razor_at, razor_clip, replace_captions, ripple_delete, ripple_trim, roll_cut,
    set_grade_at, set_in_point, set_out_point, set_track_flag, set_transform_at, slide, slip,
    snap_span, snap_to_targets, source_frame_at, toggle_grade_key, toggle_transform_key, trim,
    update_cue_text, EditError, SnapHit, SnapKind, SnapPoint, TrackFlag, TrimEdge,
};
pub use effects::{
    clip_relative, color_grade, color_grade_mut, transform, transform_mut, AnimatedF32, ColorGrade,
    Effect, GradeParam, Interpolation, KeyframeF32, Transform, TransformParam,
};
pub use model::*;
pub use session::Session;
pub use template::{builtin_templates, project_from_template, ProjectTemplate};
pub use time::{convert_frames, Frame, MediaTime, Timebase};
