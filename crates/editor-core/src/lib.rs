//! Frame-accurate editorial core for the Meridian NLE.
//!
//! Time is an integer [`Frame`] on a rational [`Timebase`]. Floating-point
//! seconds are a display and pacing convenience, never the stored edit.

pub mod caption;
pub mod compressor;
pub mod deliver;
pub mod deliver_preset;
pub mod demo;
pub mod edit;
pub mod effects;
pub mod eq;
pub mod mix;
pub mod model;
pub mod multicam;
pub mod nest;
pub mod scale;
pub mod session;
mod speed;
pub mod template;
pub mod time;

pub use caption::{
    map_words_to_cues, parse_stt_json, CaptionDraft, CaptionError, CaptionTranscriber,
    StubTranscriber, TimedWord, TranscribeRequest,
};
pub use deliver::{plan_export, ExportPlan, ExportRange};
pub use deliver_preset::{
    all_deliver_presets, builtin_deliver_presets, container_extension, custom_deliver_preset_dir,
    default_deliver_preset, load_deliver_preset_dir, load_last_deliver_settings,
    save_deliver_preset, save_last_deliver_settings, suggest_output_path, DeliverPreset,
    DeliverPresetError, DeliverSettings,
};
pub use demo::{demo_project, dense_project};
pub use edit::{
    active_sequence_id, add_adjustment_layer, add_marker, add_marker_with_color, add_title,
    add_transition, attach_proxies, create_bin, delete_bin, delete_marker,
    clip_from_media, move_media_to_bin, rename_bin, resolve_source_marks,
    collect_snap_points, delete_cue, expand_linked, import_media, insert_clips, lift_delete,
    link_clips, move_clips, overwrite_clips, razor_at, razor_clip, relink_media, replace_captions,
    ripple_delete, ripple_trim, ripple_trim_next_to_playhead, ripple_trim_prev_to_playhead,
    roll_cut, set_clip_gain_at, set_clip_speed, set_clip_title,
    set_clip_volume, set_filter_at, set_grade_at, set_in_point, set_luma_curve_point,
    set_master_fader, set_out_point, set_track_compressor, set_track_eq, set_track_eq_low_cut,
    set_track_fader, set_track_flag, set_track_pan,
    set_transform_at, set_wheel_offsets_at, slide, slip, snap_span, snap_to_targets, update_marker,
    source_frame_at, toggle_filter_key, toggle_grade_key, toggle_transform_key, toggle_volume_key,
    trim, update_cue_text,     CompressorParam, EditError, EqBand, SnapHit, SnapKind, SnapPoint, TrackFlag, TrimEdge,
};
pub use effects::{
    blur, blur_mut, chroma_key, chroma_key_mut, clip_relative, color_grade, color_grade_mut, crop,
    crop_mut, has_filter, sharpen, sharpen_mut, stabilize, stabilize_mut, transform,
    transform_mut, vignette, vignette_mut, AnimatedF32, BlurFilter, ChromaKeyFilter, ColorGrade,
    CropFilter, Effect, FilterKind, FilterParam, GradeParam, Interpolation, KeyframeF32, RgbWheel,
    SharpenFilter, StabilizeFilter, StabilizeKeyframe, ToneCurve, Transform, TransformParam,
    VignetteFilter, WheelChannel, WheelKind,
};
pub use compressor::{
    clamp_attack_ms, clamp_makeup_db, clamp_ratio, clamp_release_ms, clamp_threshold_db,
    ffmpeg_compressor_filter, format_makeup_db, format_ratio, format_threshold_db, format_time_ms,
    CompressorProcessor, TrackCompressor, ATTACK_MS_MAX, ATTACK_MS_MIN, DEFAULT_ATTACK_MS,
    DEFAULT_RELEASE_MS, MAKEUP_DB_MAX, MAKEUP_DB_MIN, RATIO_MAX, RATIO_MIN, RELEASE_MS_MAX,
    RELEASE_MS_MIN, THRESHOLD_DB_MAX, THRESHOLD_DB_MIN,
};
pub use eq::{
    clamp_eq_db, ffmpeg_eq_filters, format_eq_db, process_interleaved, EqProcessor, TrackEq3,
    EQ_DB_MAX, EQ_DB_MIN, EQ_HIGH_HZ, EQ_LOW_CUT_HZ, EQ_LOW_HZ, EQ_MID_HZ,
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
pub use multicam::{
    active_angle, angle_audio_media, angle_marks, angle_source_frame, create_multicam,
    multicam_audio_spans, multicam_target, opening_angle_name, picture_at, set_angle_sync,
    switch_angle, AnglePicture, AudibleSpan,
};
pub use nest::{
    create_nested_sequence, nested_audio_spans, nested_frame_at, nested_sequence,
    nested_sequence_id, would_cycle, MAX_NEST_DEPTH,
};
pub use scale::{
    align_frame, clamp_timeline_zoom, clip_index_at, frame_at_x, ruler_mark_count, ruler_step,
    stacked_clip_indices, stacked_hits, timeline_scale_label, timeline_x, visible_clip_span,
    visible_span, zoom_origin, RulerStep, MAX_PIXELS_PER_FRAME, MIN_PIXELS_PER_FRAME,
};
pub use session::Session;
pub use template::{builtin_templates, project_from_template, ProjectTemplate};
pub use time::{convert_frames, Frame, MediaTime, Timebase};
