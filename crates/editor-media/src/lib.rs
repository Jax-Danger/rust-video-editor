//! Media probing for Meridian.
//!
//! The default build never invokes ffmpeg. It returns a deterministic stub from
//! the file extension (and an optional `.probe.json` sidecar). Enable the
//! `ffmpeg` feature to shell out to `ffprobe`:
//!
//! ```text
//! cargo run -p editor-app --features ffmpeg
//! ```
//!
//! `ffprobe` must be on `PATH`. [`parse_ffprobe_json`] is always compiled so the
//! mapping can be tested without the binary.

mod probe;

pub use probe::{parse_ffprobe_json, probe, probe_stub, ProbeError, ProbeResult};

use editor_core::Timebase;

pub fn duration_frames(result: &ProbeResult, timebase: Timebase) -> i64 {
    result.duration.to_frame_count(timebase).max(1)
}
