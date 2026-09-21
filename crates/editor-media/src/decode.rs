//! Preview-frame requests and ffmpeg CLI decode.
//!
//! The default build validates requests and resolves paths, but never spawns a
//! process. With the `ffmpeg` feature, [`decode_frames`] shells out to the
//! `ffmpeg` binary on `PATH` and reads raw RGBA. No libav linkage.

use std::path::{Path, PathBuf};
#[cfg(feature = "ffmpeg")]
use std::process::Command;

use thiserror::Error;

/// Largest edge the preview decoder will allocate, in pixels.
pub const MAX_PREVIEW_DIMENSION: u32 = 1920;

/// Most frames one ffmpeg invocation will emit.
pub const MAX_BURST: u32 = 16;

#[derive(Clone, Debug, PartialEq)]
pub struct FrameRequest {
    pub path: String,
    /// Seconds from the start of the file. Finite and `>= 0`.
    pub time_secs: f64,
    pub width: u32,
    pub height: u32,
    /// Consecutive frames to decode, `1..=MAX_BURST`.
    pub count: u32,
}

impl FrameRequest {
    pub fn new(
        path: impl AsRef<str>,
        time_secs: f64,
        width: u32,
        height: u32,
        count: u32,
    ) -> Result<Self, DecodeError> {
        let request = Self {
            path: path.as_ref().trim().to_string(),
            time_secs,
            width,
            height,
            count,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), DecodeError> {
        if self.path.is_empty() {
            return Err(DecodeError::EmptyPath);
        }
        if !self.time_secs.is_finite() || self.time_secs < 0.0 {
            return Err(DecodeError::TimeOutOfRange);
        }
        if self.width == 0
            || self.height == 0
            || self.width > MAX_PREVIEW_DIMENSION
            || self.height > MAX_PREVIEW_DIMENSION
        {
            return Err(DecodeError::SizeOutOfRange);
        }
        if self.count == 0 || self.count > MAX_BURST {
            return Err(DecodeError::BurstOutOfRange);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Error, PartialEq)]
pub enum DecodeError {
    #[error("path is empty")]
    EmptyPath,
    #[error("time must be finite and inside the media")]
    TimeOutOfRange,
    #[error("preview size must be between 1 and {MAX_PREVIEW_DIMENSION} pixels")]
    SizeOutOfRange,
    #[error("frame count must be between 1 and {MAX_BURST}")]
    BurstOutOfRange,
    #[error("media is offline")]
    Offline,
    #[error("rebuild with --features ffmpeg to decode preview frames")]
    FeatureDisabled,
    #[error("ffmpeg is not available: {0}")]
    Unavailable(String),
    #[error("ffmpeg failed: {0}")]
    Ffmpeg(String),
    #[error("ffmpeg wrote no complete frame ({got} bytes, frame is {frame_bytes})")]
    UnexpectedSize { got: usize, frame_bytes: usize },
}

/// How picture preview is backed in this build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewBackend {
    /// Compiled without the `ffmpeg` feature. Nothing is spawned.
    Disabled,
    /// `ffmpeg` ran `-version` successfully.
    Cli,
    /// Feature is on, but the binary could not be started.
    Unavailable(String),
}

/// Reject timestamps outside `[0, duration)`.
///
/// A zero duration has no picture, so it is out of range. The end is exclusive:
/// a timestamp equal to the container duration is past the last frame.
pub fn ensure_time_in_range(time_secs: f64, duration_secs: f64) -> Result<(), DecodeError> {
    if !time_secs.is_finite()
        || !duration_secs.is_finite()
        || time_secs < 0.0
        || duration_secs <= 0.0
        || time_secs >= duration_secs
    {
        return Err(DecodeError::TimeOutOfRange);
    }
    Ok(())
}

/// Clamp a timestamp into the open interval `[0, duration)`.
///
/// A non-positive duration collapses to `0` so the caller can still build a
/// request; [`ensure_time_in_range`] is the strict check.
pub fn clamp_preview_time(time_secs: f64, duration_secs: f64) -> Result<f64, DecodeError> {
    if !time_secs.is_finite()
        || !duration_secs.is_finite()
        || time_secs < 0.0
        || duration_secs < 0.0
    {
        return Err(DecodeError::TimeOutOfRange);
    }
    if duration_secs == 0.0 {
        return Ok(0.0);
    }
    let last = (duration_secs - 0.000_5).max(0.0);
    Ok(time_secs.min(last))
}

/// Fit `src` inside `max`, preserving aspect, never upscaling, even edges.
pub fn fit_preview_size(
    src_w: u32,
    src_h: u32,
    max_w: u32,
    max_h: u32,
) -> Result<(u32, u32), DecodeError> {
    if src_w == 0
        || src_h == 0
        || max_w < 2
        || max_h < 2
        || max_w > MAX_PREVIEW_DIMENSION
        || max_h > MAX_PREVIEW_DIMENSION
    {
        return Err(DecodeError::SizeOutOfRange);
    }
    let scale = (f64::from(max_w) / f64::from(src_w))
        .min(f64::from(max_h) / f64::from(src_h))
        .min(1.0);
    let mut width = (f64::from(src_w) * scale).round() as u32;
    let mut height = (f64::from(src_h) * scale).round() as u32;
    width = (width.max(2)) & !1;
    height = (height.max(2)) & !1;
    if width > max_w {
        width = max_w & !1;
    }
    if height > max_h {
        height = max_h & !1;
    }
    if width < 2 || height < 2 {
        return Err(DecodeError::SizeOutOfRange);
    }
    Ok((width, height))
}

/// Locate `stored`, which may be relative to the working directory or the repo.
///
/// Missing paths are returned unchanged so callers can test `is_file`.
pub fn resolve_media_path(stored: &str) -> PathBuf {
    let raw = PathBuf::from(stored);
    if stored.is_empty() {
        return raw;
    }
    if raw.is_file() {
        return raw;
    }
    if let Ok(cwd) = std::env::current_dir() {
        for ancestor in cwd.ancestors() {
            let candidate = ancestor.join(&raw);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    let rooted = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(&raw);
    if rooted.is_file() {
        return rooted;
    }
    raw
}

pub fn preview_backend() -> PreviewBackend {
    #[cfg(not(feature = "ffmpeg"))]
    {
        PreviewBackend::Disabled
    }
    #[cfg(feature = "ffmpeg")]
    {
        match Command::new("ffmpeg")
            .arg("-hide_banner")
            .arg("-version")
            .output()
        {
            Ok(output) if output.status.success() => PreviewBackend::Cli,
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let message = brief(&stderr);
                PreviewBackend::Unavailable(if message.is_empty() {
                    format!("ffmpeg exited with {}", output.status)
                } else {
                    message
                })
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                PreviewBackend::Unavailable("ffmpeg was not found on PATH".into())
            }
            Err(err) => PreviewBackend::Unavailable(brief(&err.to_string())),
        }
    }
}

/// Decode `request.count` frames starting at `request.time_secs`.
///
/// Without the `ffmpeg` feature this returns [`DecodeError::FeatureDisabled`]
/// and does not spawn a process.
pub fn decode_frames(request: &FrameRequest) -> Result<Vec<DecodedFrame>, DecodeError> {
    request.validate()?;
    #[cfg(not(feature = "ffmpeg"))]
    {
        Err(DecodeError::FeatureDisabled)
    }
    #[cfg(feature = "ffmpeg")]
    {
        decode_frames_cli(request)
    }
}

#[cfg(feature = "ffmpeg")]
fn decode_frames_cli(request: &FrameRequest) -> Result<Vec<DecodedFrame>, DecodeError> {
    let path = resolve_media_path(&request.path);
    if !path.is_file() {
        return Err(DecodeError::Offline);
    }
    let time = format!("{:.6}", request.time_secs);
    let count = request.count.to_string();
    let scale = format!("scale={}:{}:flags=bilinear", request.width, request.height);
    let output = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-ss",
            &time,
            "-i",
        ])
        .arg(&path)
        .args([
            "-map",
            "0:v:0",
            "-frames:v",
            &count,
            "-an",
            "-vf",
            &scale,
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "pipe:1",
        ])
        .output()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                DecodeError::Unavailable("ffmpeg was not found on PATH".into())
            } else {
                DecodeError::Unavailable(brief(&err.to_string()))
            }
        })?;

    let frame_bytes = (request.width as usize)
        .checked_mul(request.height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(DecodeError::SizeOutOfRange)?;
    let complete = output.stdout.len() / frame_bytes;
    if complete == 0 {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = brief(&stderr);
        if !output.status.success() {
            return Err(DecodeError::Ffmpeg(if message.is_empty() {
                format!("ffmpeg exited with {}", output.status)
            } else {
                message
            }));
        }
        return Err(DecodeError::UnexpectedSize {
            got: output.stdout.len(),
            frame_bytes,
        });
    }

    let mut frames = Vec::with_capacity(complete);
    for index in 0..complete {
        let start = index * frame_bytes;
        frames.push(DecodedFrame {
            width: request.width,
            height: request.height,
            rgba: output.stdout[start..start + frame_bytes].to_vec(),
        });
    }
    Ok(frames)
}

#[cfg(feature = "ffmpeg")]
fn brief(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let mut out: String = line.chars().take(220).collect();
    if line.chars().count() > 220 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    fn frame_request_rejects_bad_bounds() {
        assert_eq!(
            FrameRequest::new("", 0.0, 16, 16, 1).unwrap_err(),
            DecodeError::EmptyPath
        );
        assert_eq!(
            FrameRequest::new("clip.mp4", -0.01, 16, 16, 1).unwrap_err(),
            DecodeError::TimeOutOfRange
        );
        assert_eq!(
            FrameRequest::new("clip.mp4", f64::NAN, 16, 16, 1).unwrap_err(),
            DecodeError::TimeOutOfRange
        );
        assert_eq!(
            FrameRequest::new("clip.mp4", f64::INFINITY, 16, 16, 1).unwrap_err(),
            DecodeError::TimeOutOfRange
        );
        assert_eq!(
            FrameRequest::new("clip.mp4", 0.0, 0, 16, 1).unwrap_err(),
            DecodeError::SizeOutOfRange
        );
        assert_eq!(
            FrameRequest::new("clip.mp4", 0.0, 16, 0, 1).unwrap_err(),
            DecodeError::SizeOutOfRange
        );
        assert_eq!(
            FrameRequest::new("clip.mp4", 0.0, MAX_PREVIEW_DIMENSION + 1, 16, 1).unwrap_err(),
            DecodeError::SizeOutOfRange
        );
        assert_eq!(
            FrameRequest::new("clip.mp4", 0.0, 16, 16, 0).unwrap_err(),
            DecodeError::BurstOutOfRange
        );
        assert_eq!(
            FrameRequest::new("clip.mp4", 0.0, 16, 16, MAX_BURST + 1).unwrap_err(),
            DecodeError::BurstOutOfRange
        );
        let ok = FrameRequest::new("  clip.mp4  ", 1.5, 32, 18, MAX_BURST).unwrap();
        assert_eq!(ok.path, "clip.mp4");
        assert_eq!(ok.count, MAX_BURST);
    }

    #[test]
    fn time_bounds_are_half_open() {
        assert_eq!(
            ensure_time_in_range(-0.1, 2.0),
            Err(DecodeError::TimeOutOfRange)
        );
        assert_eq!(
            ensure_time_in_range(f64::NAN, 2.0),
            Err(DecodeError::TimeOutOfRange)
        );
        assert_eq!(
            ensure_time_in_range(0.0, 0.0),
            Err(DecodeError::TimeOutOfRange)
        );
        assert_eq!(
            ensure_time_in_range(2.0, 2.0),
            Err(DecodeError::TimeOutOfRange)
        );
        assert_eq!(
            ensure_time_in_range(1.0, -1.0),
            Err(DecodeError::TimeOutOfRange)
        );
        assert!(ensure_time_in_range(0.0, 2.0).is_ok());
        assert!(ensure_time_in_range(1.999, 2.0).is_ok());

        assert!((clamp_preview_time(0.25, 2.0).unwrap() - 0.25).abs() < 1e-9);
        let clamped = clamp_preview_time(5.0, 2.0).unwrap();
        assert!(clamped < 2.0 && clamped > 1.9);
        assert_eq!(clamp_preview_time(1.0, 0.0).unwrap(), 0.0);
        assert_eq!(
            clamp_preview_time(-1.0, 2.0),
            Err(DecodeError::TimeOutOfRange)
        );
        assert_eq!(
            clamp_preview_time(0.0, f64::NAN),
            Err(DecodeError::TimeOutOfRange)
        );
    }

    #[test]
    fn fit_preview_size_preserves_aspect_and_stays_even() {
        assert_eq!(fit_preview_size(1920, 1080, 960, 540).unwrap(), (960, 540));
        assert_eq!(fit_preview_size(1080, 1920, 960, 540).unwrap(), (304, 540));
        assert_eq!(fit_preview_size(100, 50, 960, 540).unwrap(), (100, 50));
        assert_eq!(
            fit_preview_size(0, 10, 32, 32).unwrap_err(),
            DecodeError::SizeOutOfRange
        );
        assert_eq!(
            fit_preview_size(10, 10, 1, 10).unwrap_err(),
            DecodeError::SizeOutOfRange
        );
        assert_eq!(
            fit_preview_size(10, 10, MAX_PREVIEW_DIMENSION + 8, 32).unwrap_err(),
            DecodeError::SizeOutOfRange
        );
    }

    #[test]
    fn disabled_feature_does_not_spawn_ffmpeg() {
        let request = FrameRequest::new("missing-preview.mp4", 0.0, 16, 16, 1).unwrap();
        match preview_backend() {
            PreviewBackend::Disabled => {
                assert_eq!(
                    decode_frames(&request).unwrap_err(),
                    DecodeError::FeatureDisabled
                );
            }
            PreviewBackend::Cli => {
                assert_ne!(
                    decode_frames(&request).unwrap_err(),
                    DecodeError::FeatureDisabled
                );
            }
            PreviewBackend::Unavailable(_) => {}
        }
    }

    #[test]
    fn decodes_distinct_frames_when_ffmpeg_cli_is_available() {
        if preview_backend() != PreviewBackend::Cli {
            return;
        }
        let dir = std::env::temp_dir().join(format!("meridian-decode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bars.mp4");
        let status = Command::new("ffmpeg")
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=24:duration=1",
                "-frames:v",
                "24",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-g",
                "1",
            ])
            .arg(&path)
            .status()
            .expect("ffmpeg spawn");
        assert!(status.success(), "failed to synthesize a test clip");

        let path_str = path.to_string_lossy().into_owned();
        let start = FrameRequest::new(&path_str, 0.0, 160, 90, 1).unwrap();
        let later = FrameRequest::new(&path_str, 0.5, 160, 90, 1).unwrap();
        let first = decode_frames(&start).unwrap();
        let second = decode_frames(&later).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert_eq!(first[0].rgba.len(), 160 * 90 * 4);
        assert_ne!(first[0].rgba, second[0].rgba);

        let burst = FrameRequest::new(&path_str, 0.0, 80, 46, 4).unwrap();
        let frames = decode_frames(&burst).unwrap();
        assert_eq!(frames.len(), 4);
        assert_eq!(frames[0].width, 80);
        assert_ne!(frames[0].rgba, frames[3].rgba);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_finds_an_existing_file() {
        let dir = std::env::temp_dir().join(format!("meridian-resolve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("clip.mp4");
        std::fs::write(&file, b"not a real movie").unwrap();
        let resolved = resolve_media_path(file.to_str().unwrap());
        assert!(resolved.is_file());
        assert_eq!(resolve_media_path(""), PathBuf::from(""));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
