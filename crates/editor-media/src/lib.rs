//! Media probing and preview decode for Meridian.
//!
//! The default build never invokes ffmpeg. Probe returns a deterministic stub
//! from the file extension (and an optional `.probe.json` sidecar), and
//! [`decode_frames`] returns [`DecodeError::FeatureDisabled`]. Enable the
//! `ffmpeg` feature to shell out to `ffprobe` and `ffmpeg` on `PATH`:
//!
//! ```text
//! cargo run -p editor-app --features ffmpeg
//! ```
//!
//! Nothing in this crate links libav. [`parse_ffprobe_json`] and the frame
//! request checks are always compiled so they can be tested without the binary.

mod composite;
mod decode;
mod export;
mod frame_cache;
mod probe;
mod proxy;
#[cfg(feature = "whisper")]
mod transcribe;

pub use composite::{
    active_captions, apply_transition, burn_captions, caption_style, compose_layers, composite,
    mask_allows, mask_window, place_from_transform, program_stack, program_stack_with,
    render_title, transition_motion, video_track_visible, BlitLayer, CanvasMask, CaptionStyle,
    FilterSample, GradeSample, LayerSource, MaskWindow, Place, ProgramLayer, ProgramStack,
    TransitionMotion,
};
pub use decode::{
    clamp_preview_time, decode_audio, decode_frames, ensure_time_in_range, fit_preview_size,
    preview_backend, resolve_media_path, AudioRequest, DecodeError, DecodedFrame, FrameRequest,
    PreviewBackend, AUDIO_CHANNELS, AUDIO_RATE, MAX_AUDIO_SECONDS, MAX_BURST,
    MAX_PREVIEW_DIMENSION,
};
pub use export::{caption_font, cues_to_srt, plan_encode, FfmpegScript, WavPiece};
#[cfg(feature = "ffmpeg")]
pub use export::{spawn_export, write_timeline_wav, ExportJob, ExportSnapshot};
pub use frame_cache::{
    source_stamp, FrameCache, FrameCacheKey, DEFAULT_FRAME_CACHE_BYTES, DEFAULT_FRAME_CACHE_FILES,
};
pub use probe::{parse_ffprobe_json, probe, probe_stub, ProbeError, ProbeResult};
pub use proxy::{
    cache_root, frame_cache_dir, generate_proxy, preview_file, project_proxy_dir,
    proxy_ffmpeg_args, proxy_output_path, unsaved_proxy_dir, PreviewSource, ProxyError,
    ProxyRequest, PROXY_MAX_WIDTH,
};
#[cfg(feature = "whisper")]
pub use transcribe::{transcribe_wav, whisper_availability, WhisperPaths};

use editor_core::Timebase;

pub fn duration_frames(result: &ProbeResult, timebase: Timebase) -> i64 {
    result.duration.to_frame_count(timebase).max(1)
}
