//! Frame-accurate editorial core for the Meridian NLE.
//!
//! Time is an integer [`Frame`] on a rational [`Timebase`]. Floating-point
//! seconds are a display and pacing convenience, never the stored edit.

pub mod caption;
pub mod deliver;
pub mod demo;
pub mod edit;
pub mod effects;
pub mod mix;
pub mod model;
pub mod scale;
pub mod session;
pub mod template;
pub mod time;

pub use caption::{
    map_words_to_cues, parse_stt_json, CaptionDraft, CaptionError, CaptionTranscriber,
    StubTranscriber, TimedWord, TranscribeRequest,
};
pub use deliver::{plan_export, ExportPlan, ExportRange};
pub use demo::{demo_project, dense_project};
pub use edit::{
    active_sequence_id, add_marker, add_title, add_transition, attach_proxies, clip_from_media,
    collect_snap_points, delete_cue, expand_linked, import_media, insert_clips, lift_delete,
    link_clips, move_clips, overwrite_clips, razor_at, razor_clip, relink_media, replace_captions,
    ripple_delete, ripple_trim, roll_cut, set_clip_gain_at, set_clip_title, set_clip_volume,
    set_filter_at, set_grade_at, set_in_point, set_luma_curve_point, set_master_fader, set_out_point,
    set_track_fader, set_track_flag, set_track_pan, set_transform_at, set_wheel_offsets_at, slide,
    slip, snap_span, snap_to_targets, source_frame_at, toggle_filter_key, toggle_grade_key,
    toggle_transform_key, toggle_volume_key, trim, update_cue_text, EditError, SnapHit, SnapKind,
    SnapPoint, TrackFlag, TrimEdge,
};
pub use effects::{
    blur, blur_mut, clip_relative, color_grade, color_grade_mut, crop, crop_mut, has_filter,
    sharpen, sharpen_mut, transform, transform_mut, vignette, vignette_mut, AnimatedF32,
    BlurFilter, ColorGrade, CropFilter, Effect, FilterKind, FilterParam, GradeParam, Interpolation,
    KeyframeF32, RgbWheel, SharpenFilter, ToneCurve, Transform, TransformParam, VignetteFilter,
    WheelChannel, WheelKind,
};
pub use mix::{
    accumulate_stereo, audio_solo_active, audio_topology, channel_clips, clamp_gain, clamp_pan,
    clip_gain_curve, db_to_linear, fader_pos_to_linear, ffmpeg_pan_filter, ffmpeg_volume_arg,
    format_db, format_pan, linear_to_db, linear_to_fader_pos, measure_stereo, meter_amount,
    mix_frame, mix_regions, pan_gains, scaled_curve, stereo_frame, track_is_audible, update_hold,
    BusClip, BusState, BusTrack, GainCurve, GainKey, Level, MixFrame, MixRegion, FADER_DB_MAX,
    FADER_DB_MIN, GAIN_MAX,
};
pub use model::*;
pub use scale::{
    align_frame, clamp_timeline_zoom, clip_index_at, frame_at_x, ruler_mark_count, ruler_step,
    stacked_clip_indices, stacked_hits, timeline_scale_label, timeline_x, visible_clip_span,
    visible_span, zoom_origin, RulerStep, MAX_PIXELS_PER_FRAME, MIN_PIXELS_PER_FRAME,
};
pub use session::Session;
pub use template::{builtin_templates, project_from_template, ProjectTemplate};
pub use time::{convert_frames, Frame, MediaTime, Timebase};
