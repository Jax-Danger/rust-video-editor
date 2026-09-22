//! Optimized-media proxies.
//!
//! A proxy is a 960-wide (or narrower) H.264 file next to the project. Preview
//! can prefer it and falls back to the original when the file is missing.
//! Export always decodes the original. Nothing here links libav; generation
//! shells out to `ffmpeg` only when the `ffmpeg` feature is on.

use std::path::{Path, PathBuf};
#[cfg(any(feature = "ffmpeg", test))]
use std::process::Command;

use editor_core::MediaAsset;
use thiserror::Error;

/// Longest edge of a generated proxy, in pixels. Sources that are already
/// narrower stay at their own width.
pub const PROXY_MAX_WIDTH: u32 = 960;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewSource {
    /// Always the camera original. Export uses this.
    Full,
    /// The proxy file when it is on disk, otherwise the original.
    Proxy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyRequest {
    pub source: PathBuf,
    pub output: PathBuf,
    pub max_width: u32,
}

impl ProxyRequest {
    pub fn standard(source: impl Into<PathBuf>, output: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
            output: output.into(),
            max_width: PROXY_MAX_WIDTH,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProxyError {
    #[error("proxy source is missing")]
    Offline,
    #[error("rebuild with --features ffmpeg to generate proxies")]
    FeatureDisabled,
    #[error("ffmpeg is not available: {0}")]
    Unavailable(String),
    #[error("ffmpeg failed: {0}")]
    Ffmpeg(String),
}

/// Cache root. `XDG_CACHE_HOME/meridian`, else `~/.cache/meridian`.
pub fn cache_root() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        let trimmed = xdg.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join("meridian");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join(".cache").join("meridian");
        }
    }
    std::env::temp_dir().join("meridian-cache")
}

pub fn frame_cache_dir() -> PathBuf {
    cache_root().join("frames")
}

/// Proxies for a project that has not been saved yet.
pub fn unsaved_proxy_dir() -> PathBuf {
    cache_root().join("proxies")
}

/// `<project stem>.meridian/proxies` beside the project file.
pub fn project_proxy_dir(project_file: &Path) -> PathBuf {
    let parent = project_file.parent().unwrap_or_else(|| Path::new("."));
    let stem = project_file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("project");
    parent.join(format!("{stem}.meridian")).join("proxies")
}

pub fn proxy_output_path(dir: &Path, source: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
    let hash = fnv1a(canonical.to_string_lossy().as_bytes());
    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("media");
    let stem: String = stem
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .take(40)
        .collect();
    let stem = if stem.is_empty() { "media" } else { &stem };
    dir.join(format!("{stem}-{hash:016x}-w{PROXY_MAX_WIDTH}.mp4"))
}

/// Resolved preview path and whether it is the proxy rather than the original.
pub fn preview_file(asset: &MediaAsset, source: PreviewSource) -> (PathBuf, bool) {
    if source == PreviewSource::Proxy {
        if let Some(path) = asset
            .proxy_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            let resolved = crate::resolve_media_path(path);
            if resolved.is_file() {
                return (resolved, true);
            }
        }
    }
    (crate::resolve_media_path(&asset.path), false)
}

/// ffmpeg arguments for a proxy. The comma inside `min()` is escaped so the
/// filtergraph does not split.
pub fn proxy_ffmpeg_args(request: &ProxyRequest) -> Vec<String> {
    let width = request.max_width.clamp(2, PROXY_MAX_WIDTH) & !1;
    // The comma is escaped so ffmpeg does not treat it as a filter separator.
    let filter = format!("scale=w=min({width}\\,iw):h=-2:flags=bilinear");
    vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-i".into(),
        request.source.to_string_lossy().into_owned(),
        "-vf".into(),
        filter,
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        "23".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-g".into(),
        "12".into(),
        "-bf".into(),
        "0".into(),
        "-an".into(),
        "-movflags".into(),
        "+faststart".into(),
        request.output.to_string_lossy().into_owned(),
    ]
}

pub fn generate_proxy(request: &ProxyRequest) -> Result<(), ProxyError> {
    if request.source.as_os_str().is_empty() || !request.source.is_file() {
        return Err(ProxyError::Offline);
    }
    #[cfg(not(feature = "ffmpeg"))]
    {
        let _ = request;
        Err(ProxyError::FeatureDisabled)
    }
    #[cfg(feature = "ffmpeg")]
    {
        generate_proxy_cli(request)
    }
}

#[cfg(feature = "ffmpeg")]
fn generate_proxy_cli(request: &ProxyRequest) -> Result<(), ProxyError> {
    if let Some(parent) = request.output.parent() {
        std::fs::create_dir_all(parent).map_err(|err| ProxyError::Ffmpeg(err.to_string()))?;
    }
    let args = proxy_ffmpeg_args(request);
    let output = Command::new("ffmpeg").args(&args).output().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            ProxyError::Unavailable("ffmpeg was not found on PATH".into())
        } else {
            ProxyError::Unavailable(err.to_string())
        }
    })?;
    if !output.status.success() || !request.output.is_file() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let line = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("");
        return Err(ProxyError::Ffmpeg(if line.is_empty() {
            format!("ffmpeg exited with {}", output.status)
        } else {
            line.chars().take(220).collect()
        }));
    }
    Ok(())
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::{BinId, Frame, MediaId, Timebase};

    fn asset(path: &str, proxy: Option<&str>) -> MediaAsset {
        MediaAsset {
            id: MediaId(1),
            bin_id: BinId(1),
            name: "clip".into(),
            path: path.into(),
            duration: Frame(24),
            timebase: Timebase::fps_24(),
            width: Some(1920),
            height: Some(1080),
            video_codec: Some("h264".into()),
            audio_codec: None,
            audio_channels: None,
            sample_rate: None,
            has_video: true,
            has_audio: false,
            offline: false,
            proxy_path: proxy.map(str::to_string),
        }
    }

    #[test]
    fn proxy_args_target_960_wide_h264() {
        let request = ProxyRequest::standard("camera.mov", "/cache/camera.mp4");
        assert_eq!(request.max_width, 960);
        let args = proxy_ffmpeg_args(&request);
        let filter = args[args.iter().position(|arg| arg == "-vf").unwrap() + 1].clone();
        assert!(filter.contains("min(960\\,iw)"), "{filter}");
        assert!(args.iter().any(|arg| arg == "libx264"));
        assert!(args.iter().any(|arg| arg == "-an"));
        assert_eq!(args.last().unwrap(), "/cache/camera.mp4");
    }

    #[test]
    fn proxy_path_is_stable_and_preview_falls_back() {
        let dir = std::env::temp_dir().join(format!("meridian-proxy-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("interview.mov");
        std::fs::write(&source, b"original").unwrap();
        let cache = dir.join("proxies");
        let output = proxy_output_path(&cache, &source);
        assert_eq!(output, proxy_output_path(&cache, &source));
        assert!(output.starts_with(&cache));
        assert!(output
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("interview-"));

        let missing = asset(source.to_str().unwrap(), Some("/no/such/proxy.mp4"));
        let (resolved, using_proxy) = preview_file(&missing, PreviewSource::Proxy);
        assert!(!using_proxy);
        assert_eq!(resolved, source);

        let (full, using_proxy) = preview_file(&missing, PreviewSource::Full);
        assert!(!using_proxy);
        assert_eq!(full, source);

        let proxy = dir.join("ready.mp4");
        std::fs::write(&proxy, b"proxy").unwrap();
        let ready = asset(source.to_str().unwrap(), proxy.to_str());
        let (resolved, using_proxy) = preview_file(&ready, PreviewSource::Proxy);
        assert!(using_proxy);
        assert_eq!(resolved, proxy);
        let (full, using_proxy) = preview_file(&ready, PreviewSource::Full);
        assert!(!using_proxy);
        assert_eq!(full, source);

        let project = dir.join("northline.json");
        let beside = project_proxy_dir(&project);
        assert!(beside.ends_with("northline.meridian/proxies"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_source_is_offline_without_spawning() {
        let request = ProxyRequest::standard("missing-proxy-source.mp4", "/tmp/out.mp4");
        assert_eq!(generate_proxy(&request).unwrap_err(), ProxyError::Offline);
    }

    #[test]
    fn feature_disabled_does_not_spawn() {
        if cfg!(feature = "ffmpeg") {
            return;
        }
        let dir = std::env::temp_dir().join(format!("meridian-proxy-off-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("clip.mp4");
        std::fs::write(&source, b"not a movie").unwrap();
        let request = ProxyRequest::standard(&source, dir.join("out.mp4"));
        assert_eq!(
            generate_proxy(&request).unwrap_err(),
            ProxyError::FeatureDisabled
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generates_a_narrow_h264_proxy_when_ffmpeg_is_available() {
        if !cfg!(feature = "ffmpeg") {
            return;
        }
        if Command::new("ffmpeg")
            .arg("-hide_banner")
            .arg("-version")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .is_none()
        {
            return;
        }
        let dir = std::env::temp_dir().join(format!("meridian-proxy-gen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("wide.mp4");
        let status = Command::new("ffmpeg")
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=640x360:rate=24:duration=0.2",
                "-frames:v",
                "4",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&source)
            .status()
            .expect("ffmpeg");
        assert!(status.success());
        let output = dir.join("wide-proxy.mp4");
        let request = ProxyRequest {
            source: source.clone(),
            output: output.clone(),
            max_width: 320,
        };
        generate_proxy(&request).unwrap();
        assert!(output.is_file());
        assert!(output.metadata().unwrap().len() > 32);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
