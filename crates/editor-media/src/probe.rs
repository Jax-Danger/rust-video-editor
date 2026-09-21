//! Probe implementations: stub (default) and ffprobe JSON.

use std::path::{Path, PathBuf};
#[cfg(feature = "ffmpeg")]
use std::process::Command;

use editor_core::{MediaTime, Timebase};
use serde::Deserialize;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeResult {
    pub path: String,
    pub duration: MediaTime,
    pub timebase: Option<Timebase>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub audio_channels: Option<u32>,
    pub sample_rate: Option<u32>,
    pub has_video: bool,
    pub has_audio: bool,
    pub offline: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProbeError {
    #[error("path is empty")]
    EmptyPath,
    #[error("ffprobe failed: {0}")]
    Ffprobe(String),
    #[error("ffprobe returned no streams")]
    NoStreams,
    #[error("could not parse ffprobe output: {0}")]
    Parse(String),
    #[error("sidecar probe is invalid: {0}")]
    Sidecar(String),
}

/// Probe `path`.
///
/// With the `ffmpeg` feature this runs `ffprobe`. Otherwise it uses the stub.
pub fn probe(path: &Path) -> Result<ProbeResult, ProbeError> {
    #[cfg(feature = "ffmpeg")]
    {
        probe_ffprobe(path)
    }
    #[cfg(not(feature = "ffmpeg"))]
    {
        probe_stub(path)
    }
}

pub fn probe_stub(path: &Path) -> Result<ProbeResult, ProbeError> {
    if path.as_os_str().is_empty() {
        return Err(ProbeError::EmptyPath);
    }
    if let Some(sidecar) = read_sidecar(path)? {
        return Ok(sidecar);
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("media")
        .to_string();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let offline = !path.exists();
    let (has_video, has_audio, video_codec, audio_codec) = classify(&ext);
    let timebase = infer_timebase(&name, &ext, has_video);
    let seconds = stable_seconds(&name);
    // Stills get a 5-second hold so they can be cut onto a timeline.
    let frames = if is_still(&ext) {
        timebase.timecode_fps() * 5
    } else {
        seconds * timebase.timecode_fps().max(1)
    };
    let (width, height) = if has_video {
        infer_dimension(&name)
    } else {
        (None, None)
    };
    Ok(ProbeResult {
        path: path.to_string_lossy().into_owned(),
        duration: MediaTime::from_frames(frames.max(1), timebase),
        timebase: Some(timebase),
        width,
        height,
        video_codec,
        audio_codec,
        audio_channels: has_audio.then_some(2),
        sample_rate: has_audio.then_some(48_000),
        has_video,
        has_audio,
        offline,
    })
}

#[cfg(feature = "ffmpeg")]
pub fn probe_ffprobe(path: &Path) -> Result<ProbeResult, ProbeError> {
    if path.as_os_str().is_empty() {
        return Err(ProbeError::EmptyPath);
    }
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .map_err(|err| ProbeError::Ffprobe(err.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProbeError::Ffprobe(stderr.trim().to_string()));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut result = parse_ffprobe_json(&text)?;
    result.path = path.to_string_lossy().into_owned();
    result.offline = !path.exists();
    Ok(result)
}

#[cfg(not(feature = "ffmpeg"))]
#[allow(dead_code)]
fn probe_ffprobe(path: &Path) -> Result<ProbeResult, ProbeError> {
    let _ = path;
    Err(ProbeError::Ffprobe(
        "rebuild editor-media with --features ffmpeg to call ffprobe".into(),
    ))
}

pub fn parse_ffprobe_json(text: &str) -> Result<ProbeResult, ProbeError> {
    let parsed: FfprobeOutput =
        serde_json::from_str(text).map_err(|err| ProbeError::Parse(err.to_string()))?;
    if parsed.streams.is_empty() {
        return Err(ProbeError::NoStreams);
    }
    let mut video = None;
    let mut audio = None;
    for stream in &parsed.streams {
        match stream.codec_type.as_deref() {
            Some("video") if video.is_none() && stream.codec_name.as_deref() != Some("mjpeg") => {
                video = Some(stream);
            }
            Some("audio") if audio.is_none() => audio = Some(stream),
            _ => {}
        }
    }
    // Attached pictures are ignored above; fall back to the first video stream.
    if video.is_none() {
        video = parsed
            .streams
            .iter()
            .find(|s| s.codec_type.as_deref() == Some("video"));
    }
    let timebase = video.and_then(|stream| parse_rate(stream.r_frame_rate.as_deref()));
    let timebase = timebase.unwrap_or_else(|| {
        if audio.is_some() {
            let rate = audio
                .and_then(|s| s.sample_rate.as_deref())
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(48_000);
            Timebase::new(rate, 1)
        } else {
            Timebase::fps_24()
        }
    });
    let seconds = parsed
        .format
        .as_ref()
        .and_then(|f| f.duration.as_deref())
        .and_then(|d| d.parse::<f64>().ok())
        .unwrap_or(0.0);
    let ticks = (seconds * 1_000_000.0).round() as i64;
    let duration = MediaTime::new(ticks.max(0), 1_000_000);
    Ok(ProbeResult {
        path: String::new(),
        duration,
        timebase: Some(timebase),
        width: video.and_then(|s| s.width),
        height: video.and_then(|s| s.height),
        video_codec: video.and_then(|s| s.codec_name.clone()),
        audio_codec: audio.and_then(|s| s.codec_name.clone()),
        audio_channels: audio.and_then(|s| s.channels),
        sample_rate: audio
            .and_then(|s| s.sample_rate.as_deref())
            .and_then(|s| s.parse().ok()),
        has_video: video.is_some(),
        has_audio: audio.is_some(),
        offline: false,
    })
}

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    streams: Vec<FfStream>,
    format: Option<FfFormat>,
}

#[derive(Debug, Deserialize)]
struct FfStream {
    codec_name: Option<String>,
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    r_frame_rate: Option<String>,
    channels: Option<u32>,
    sample_rate: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FfFormat {
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SidecarProbe {
    duration_ticks: i64,
    timescale: u32,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    video_codec: Option<String>,
    #[serde(default)]
    audio_codec: Option<String>,
    #[serde(default)]
    audio_channels: Option<u32>,
    #[serde(default)]
    sample_rate: Option<u32>,
    fps_num: u32,
    fps_den: u32,
    #[serde(default)]
    has_video: bool,
    #[serde(default)]
    has_audio: bool,
}

fn read_sidecar(path: &Path) -> Result<Option<ProbeResult>, ProbeError> {
    let mut sidecar: PathBuf = path.to_path_buf();
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => format!("{name}.probe.json"),
        None => return Ok(None),
    };
    sidecar.set_file_name(name);
    if !sidecar.exists() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(&sidecar).map_err(|err| ProbeError::Sidecar(err.to_string()))?;
    let parsed: SidecarProbe =
        serde_json::from_str(&text).map_err(|err| ProbeError::Sidecar(err.to_string()))?;
    let timebase = Timebase::new(parsed.fps_num, parsed.fps_den);
    Ok(Some(ProbeResult {
        path: path.to_string_lossy().into_owned(),
        duration: MediaTime::new(parsed.duration_ticks, parsed.timescale.max(1)),
        timebase: Some(timebase),
        width: parsed.width,
        height: parsed.height,
        video_codec: parsed.video_codec,
        audio_codec: parsed.audio_codec,
        audio_channels: parsed.audio_channels,
        sample_rate: parsed.sample_rate,
        has_video: parsed.has_video,
        has_audio: parsed.has_audio,
        offline: !path.exists(),
    }))
}

fn classify(ext: &str) -> (bool, bool, Option<String>, Option<String>) {
    match ext {
        "mp4" | "mov" | "mxf" | "mkv" | "avi" | "webm" | "m4v" => (
            true,
            true,
            Some(
                match ext {
                    "webm" => "vp9",
                    "mkv" => "h265",
                    _ => "h264",
                }
                .into(),
            ),
            Some("aac".into()),
        ),
        "wav" | "aif" | "aiff" | "mp3" | "flac" | "m4a" | "aac" => {
            (false, true, None, Some(ext.into()))
        }
        "png" | "jpg" | "jpeg" | "tif" | "tiff" | "exr" | "dpx" => {
            (true, false, Some(ext.into()), None)
        }
        _ => (true, true, Some("h264".into()), Some("aac".into())),
    }
}

fn is_still(ext: &str) -> bool {
    matches!(ext, "png" | "jpg" | "jpeg" | "tif" | "tiff" | "exr" | "dpx")
}

fn infer_timebase(name: &str, ext: &str, has_video: bool) -> Timebase {
    if !has_video {
        return Timebase::new(48_000, 1);
    }
    if is_still(ext) {
        return Timebase::fps_24();
    }
    let lower = name.to_ascii_lowercase();
    if lower.contains("5994") || lower.contains("59.94") {
        Timebase::fps_5994()
    } else if lower.contains("2997") || lower.contains("29.97") {
        Timebase::fps_2997()
    } else if lower.contains("2398") || lower.contains("23.976") {
        Timebase::fps_23976()
    } else if lower.contains("60") {
        Timebase::fps_60()
    } else if lower.contains("25") {
        Timebase::fps_25()
    } else if lower.contains("30") {
        Timebase::fps_30()
    } else {
        Timebase::fps_24()
    }
}

fn infer_dimension(name: &str) -> (Option<u32>, Option<u32>) {
    let lower = name.to_ascii_lowercase();
    if lower.contains("3840")
        || lower.contains("2160")
        || lower.contains("uhd")
        || lower.contains("4k")
    {
        (Some(3840), Some(2160))
    } else if lower.contains("1280") || lower.contains("720p") {
        (Some(1280), Some(720))
    } else if lower.contains("vertical") || lower.contains("9x16") {
        (Some(1080), Some(1920))
    } else {
        (Some(1920), Some(1080))
    }
}

fn stable_seconds(name: &str) -> i64 {
    let mut hash: u64 = 14_695_981_039_346_656_037;
    for byte in name.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    8 + (hash % 52) as i64
}

fn parse_rate(rate: Option<&str>) -> Option<Timebase> {
    let rate = rate?;
    let (num, den) = rate.split_once('/')?;
    let num: u32 = num.parse().ok()?;
    let den: u32 = den.parse().ok()?;
    if num == 0 || den == 0 {
        return None;
    }
    Some(Timebase::new(num, den))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_classifies_extensions() {
        let video = probe_stub(Path::new("interview_1080.mp4")).unwrap();
        assert!(video.has_video && video.has_audio);
        assert_eq!(video.video_codec.as_deref(), Some("h264"));
        assert_eq!(video.width, Some(1920));
        assert_eq!(video.timebase, Some(Timebase::fps_24()));
        assert!(video.offline);
        assert!(video.duration.to_frame_count(Timebase::fps_24()) >= 24 * 8);

        let audio = probe_stub(Path::new("tone.wav")).unwrap();
        assert!(!audio.has_video && audio.has_audio);
        assert_eq!(audio.timebase, Some(Timebase::new(48_000, 1)));
        assert!(audio.width.is_none());

        let still = probe_stub(Path::new("slate.png")).unwrap();
        assert!(still.has_video && !still.has_audio);
        assert_eq!(still.duration.to_frame_count(Timebase::fps_24()), 24 * 5);
    }

    #[test]
    fn stub_is_stable_for_the_same_name() {
        let a = probe_stub(Path::new("city.mov")).unwrap();
        let b = probe_stub(Path::new("city.mov")).unwrap();
        assert_eq!(a.duration, b.duration);
    }

    #[test]
    fn ffprobe_json_maps_ntsc_and_duration() {
        let json = r#"
        {
          "streams": [
            {
              "codec_type": "video",
              "codec_name": "h264",
              "width": 1920,
              "height": 1080,
              "r_frame_rate": "24000/1001"
            },
            {
              "codec_type": "audio",
              "codec_name": "aac",
              "channels": 2,
              "sample_rate": "48000"
            }
          ],
          "format": { "duration": "12.5" }
        }
        "#;
        let probe = parse_ffprobe_json(json).unwrap();
        assert_eq!(probe.timebase, Some(Timebase::fps_23976()));
        assert_eq!(probe.width, Some(1920));
        assert_eq!(probe.audio_codec.as_deref(), Some("aac"));
        assert_eq!(probe.sample_rate, Some(48_000));
        let frames = probe.duration.to_frame_count(Timebase::fps_23976());
        assert_eq!(frames, 300);
        assert!(probe.has_video && probe.has_audio);
    }

    #[test]
    fn empty_path_errors() {
        assert_eq!(probe_stub(Path::new("")), Err(ProbeError::EmptyPath));
    }
}
