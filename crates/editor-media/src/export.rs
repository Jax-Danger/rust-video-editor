//! Timeline export.
//!
//! [`plan_encode`] is pure: it plans one CPU composite per frame with the same
//! grade, transform, and transitions as the program monitor, plus an ffmpeg
//! audio graph. It does not spawn anything, so `cargo test` can check the stack
//! without the `ffmpeg` feature. With that feature, [`spawn_export`] decodes
//! the planned layers, runs [`composite`], burns captions, and pipes the
//! pictures to ffmpeg for the codec.

#[cfg(feature = "ffmpeg")]
use std::path::Path;
use std::path::PathBuf;

use editor_core::{
    ffmpeg_pan_filter, ffmpeg_volume_arg, mix_regions, source_frame_at, ExportRange, Frame,
    GainCurve, MediaAsset, Sequence, Timebase, TrackKind,
};

use crate::composite::{active_captions, program_stack, ProgramLayer};
#[cfg(feature = "ffmpeg")]
use crate::composite::{compose_layers, LayerSource};

/// Optional encoder overrides from a deliver preset or the panel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EncodeHints {
    pub video_bitrate_kbps: Option<u32>,
    pub audio_bitrate_kbps: Option<u32>,
    pub audio_only: bool,
}

/// One audible region. Times are sequence frames; `source_at_in` is seconds
/// into the media at `timeline_in`.
#[derive(Clone, Debug)]
pub struct WavPiece {
    pub path: String,
    pub timeline_in: i64,
    pub timeline_out: i64,
    pub source_at_in: f64,
    pub seconds_per_frame: f64,
    /// Clip gain × track fader at the clip in-point. Used when `gain_keys` is empty.
    pub gain: f32,
    /// Track pan, −1 left … +1 right. Center omits the pan filter.
    pub pan: f32,
    /// Keyed clip gain × track fader, in sequence frames.
    pub gain_keys: Vec<editor_core::GainKey>,
}

#[derive(Clone, Debug)]
pub struct FfmpegScript {
    pub inputs: Vec<String>,
    pub filter: String,
    pub video_label: String,
    pub audio_label: String,
    pub srt: String,
    pub duration_secs: f64,
    pub codec_args: Vec<String>,
    pub container: String,
    pub warnings: Vec<String>,
    pub burn_captions: bool,
    pub audio_only: bool,
    /// Picture is rasterized with the shared compositor, then encoded.
    pub raster: RasterPlan,
}

/// One output frame. Layers are bottom to top, already graded in their `place`.
#[derive(Clone, Debug)]
pub struct RasterFrame {
    pub layers: Vec<ProgramLayer>,
    pub captions: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct RasterPlan {
    pub width: u32,
    pub height: u32,
    pub seq_width: f32,
    pub seq_height: f32,
    pub fps: String,
    pub frames: Vec<RasterFrame>,
}

#[derive(Clone, Debug)]
#[cfg_attr(not(feature = "ffmpeg"), allow(dead_code))]
pub struct ExportSnapshot {
    pub fraction: f32,
    pub message: String,
    pub finished: bool,
    pub ok: bool,
}

pub fn plan_encode(
    sequence: &Sequence,
    media: &[MediaAsset],
    range: ExportRange,
    codec: &str,
    container: &str,
    burn_captions: bool,
    hints: EncodeHints,
) -> Result<FfmpegScript, String> {
    let (range_in, range_out) = export_bounds(sequence, range)?;
    let mut warnings = Vec::new();
    let audio_only = hints.audio_only || is_audio_only_codec(codec, container);
    if audio_only && burn_captions {
        warnings.push(
            "Captions are not burned into an audio-only export; use a video preset instead."
                .into(),
        );
    }
    let width = even_dim(sequence.width);
    let height = even_dim(sequence.height);
    let fps = fps_token(sequence.timebase);
    let pieces = audible_pieces(sequence, media, range_in, range_out, &mut warnings);
    let dur = frames_secs(range_out - range_in, sequence.timebase);
    let mut graph = Graph::default();
    let audio = slice_audio(
        &mut graph,
        &pieces,
        range_in,
        range_out,
        sequence.timebase,
        dur,
        sequence.master_fader,
    );

    let burn = burn_captions && !audio_only;
    let mut frames = Vec::with_capacity((range_out - range_in) as usize);
    if !audio_only {
        for frame in range_in..range_out {
            let stack = program_stack(sequence, media, frame, width, height);
            if let Some(error) = stack.errors.first() {
                return Err(error.clone());
            }
            let captions = if burn {
                active_captions(sequence, frame)
            } else {
                Vec::new()
            };
            frames.push(RasterFrame {
                layers: stack.layers,
                captions,
            });
        }
        if frames.is_empty() {
            return Err("nothing to export".into());
        }
    } else if pieces.is_empty() {
        return Err("no audible audio in the export range".into());
    }

    let cues = caption_cues(sequence, range_in, range_out);
    let srt = cues_to_srt(&cues);

    Ok(FfmpegScript {
        inputs: graph.inputs,
        filter: graph.filters.join(";"),
        video_label: "raster".into(),
        audio_label: audio,
        srt,
        duration_secs: dur,
        codec_args: codec_args(codec, container, &hints),
        container: container_name(container).to_string(),
        warnings,
        burn_captions: burn,
        audio_only,
        raster: RasterPlan {
            width,
            height,
            seq_width: sequence.width.max(1) as f32,
            seq_height: sequence.height.max(1) as f32,
            fps,
            frames,
        },
    })
}

pub fn cues_to_srt(cues: &[(f64, f64, String)]) -> String {
    let mut out = String::new();
    for (index, (start, end, text)) in cues.iter().enumerate() {
        if *end <= *start || text.trim().is_empty() {
            continue;
        }
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            index + 1,
            srt_time(*start),
            srt_time(*end),
            text.trim()
        ));
    }
    out
}

pub fn caption_font() -> Option<PathBuf> {
    [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/liberation-sans/LiberationSans-Regular.ttf",
        "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
        "/Library/Fonts/Arial.ttf",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "C:\\Windows\\Fonts\\arial.ttf",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
}

#[cfg(feature = "ffmpeg")]
pub fn write_timeline_wav(
    pieces: &[WavPiece],
    range_in: i64,
    range_out: i64,
    timebase: Timebase,
    dest: &Path,
) -> Result<(), String> {
    if pieces.is_empty() || range_out <= range_in {
        return Err("no audible audio in the caption range".into());
    }
    let dur = frames_secs(range_out - range_in, timebase);
    let mut graph = Graph::default();
    let audio = slice_audio(&mut graph, pieces, range_in, range_out, timebase, dur, 1.0);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let filter = graph.filters.join(";");
    let mut command = std::process::Command::new("ffmpeg");
    command
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin");
    for input in &graph.inputs {
        command.arg("-i").arg(input);
    }
    let status = command
        .arg("-filter_complex")
        .arg(&filter)
        .arg("-map")
        .arg(format!("[{audio}]"))
        .args(["-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
        .arg(dest)
        .status()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                "ffmpeg was not found on PATH".to_string()
            } else {
                err.to_string()
            }
        })?;
    if !status.success() {
        return Err(format!("ffmpeg could not extract caption audio ({status})"));
    }
    if !dest.is_file() {
        return Err("caption audio was not written".into());
    }
    Ok(())
}

#[cfg(feature = "ffmpeg")]
pub struct ExportJob {
    shared: std::sync::Arc<std::sync::Mutex<ExportSnapshot>>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(feature = "ffmpeg")]
impl ExportJob {
    pub fn snapshot(&self) -> ExportSnapshot {
        self.shared
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    pub fn cancel(&self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(feature = "ffmpeg")]
pub fn spawn_export(script: FfmpegScript, output: PathBuf) -> Result<ExportJob, String> {
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
    }
    let shared = std::sync::Arc::new(std::sync::Mutex::new(ExportSnapshot {
        fraction: 0.0,
        message: format!("Encoding {}", output.display()),
        finished: false,
        ok: false,
    }));
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let job = ExportJob {
        shared: shared.clone(),
        cancel: cancel.clone(),
    };
    std::thread::spawn(move || {
        let result = encode_blocking(&script, &output, &cancel, &shared);
        let mut slot = shared.lock().unwrap_or_else(|poison| poison.into_inner());
        match result {
            Ok(()) => {
                slot.fraction = 1.0;
                slot.ok = true;
                slot.finished = true;
                let mut message = format!("Wrote {}", output.display());
                if !script.warnings.is_empty() {
                    message.push_str("\n");
                    message.push_str(&script.warnings.join("\n"));
                }
                slot.message = message;
            }
            Err(err) => {
                slot.ok = false;
                slot.finished = true;
                slot.message = err;
                let _ = std::fs::remove_file(&output);
            }
        }
    });
    Ok(job)
}

#[cfg(feature = "ffmpeg")]
fn encode_blocking(
    script: &FfmpegScript,
    output: &Path,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    shared: &std::sync::Arc<std::sync::Mutex<ExportSnapshot>>,
) -> Result<(), String> {
    if script.audio_only {
        return encode_audio_only_blocking(script, output, cancel, shared);
    }
    let temp = std::env::temp_dir().join(format!(
        "meridian-export-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|dur| dur.as_millis())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&temp).map_err(|err| err.to_string())?;
    let srt_path = temp.join("captions.srt");
    let soft = !script.srt.trim().is_empty() && matches!(script.container.as_str(), "mp4" | "mov");
    if soft || (script.burn_captions && !script.srt.trim().is_empty()) {
        std::fs::write(&srt_path, &script.srt).map_err(|err| err.to_string())?;
    }

    let video_index = script.inputs.len();
    let mut command = std::process::Command::new("ffmpeg");
    command
        .arg("-y")
        .arg("-hide_banner")
        .arg("-nostats")
        .arg("-stats_period")
        .arg("0.25")
        .arg("-progress")
        .arg("pipe:1");
    for input in &script.inputs {
        command.arg("-i").arg(input);
    }
    command
        .arg("-f")
        .arg("rawvideo")
        .arg("-pix_fmt")
        .arg("rgba")
        .arg("-s")
        .arg(format!("{}x{}", script.raster.width, script.raster.height))
        .arg("-r")
        .arg(&script.raster.fps)
        .arg("-i")
        .arg("pipe:0");
    let srt_index = video_index + 1;
    if soft {
        command.arg("-i").arg(&srt_path);
    }
    command
        .arg("-filter_complex")
        .arg(&script.filter)
        .arg("-map")
        .arg(format!("{video_index}:v"))
        .arg("-map")
        .arg(format!("[{}]", script.audio_label));
    if soft {
        command.arg("-map").arg(format!("{srt_index}:0"));
    }
    command.args(&script.codec_args);
    if soft {
        command.args(["-c:s", "mov_text", "-metadata:s:s:0", "language=eng"]);
    }
    command
        .arg("-frames:v")
        .arg(script.raster.frames.len().to_string())
        .arg("-f")
        .arg(&script.container)
        .arg(output);
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            "ffmpeg was not found on PATH".to_string()
        } else {
            err.to_string()
        }
    })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "ffmpeg stdin was not piped".to_string())?;
    let raster = script.raster.clone();
    let cancel_write = cancel.clone();
    let shared_write = shared.clone();
    let writer = std::thread::spawn(move || {
        let result = write_raster(&mut stdin, &raster, &cancel_write, &shared_write);
        drop(stdin);
        result
    });
    let stderr = child.stderr.take();
    let stderr_handle = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut pipe) = stderr {
            use std::io::Read;
            let _ = pipe.read_to_string(&mut text);
        }
        text
    });
    let cancel_flag = cancel.clone();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_flag = stop.clone();
    let pid = child.id();
    let killer = std::thread::spawn(move || {
        while !stop_flag.load(std::sync::atomic::Ordering::Relaxed) {
            if cancel_flag.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = std::process::Command::new("kill")
                    .args(["-TERM", &pid.to_string()])
                    .status();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
    });

    if let Some(stdout) = child.stdout.take() {
        use std::io::{BufRead, BufReader};
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = child.kill();
                break;
            }
            if let Some(secs) = progress_seconds(&line) {
                let fraction = if script.duration_secs > 0.0 {
                    (secs / script.duration_secs).clamp(0.0, 0.99) as f32
                } else {
                    0.0
                };
                if let Ok(mut slot) = shared.lock() {
                    slot.fraction = fraction;
                    slot.message = format!("Encoding {:.0}%", fraction * 100.0);
                }
            }
        }
    }
    let status = child.wait().map_err(|err| err.to_string())?;
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = killer.join();
    let stderr_text = stderr_handle.join().unwrap_or_default();
    let write_result = writer
        .join()
        .unwrap_or_else(|_| Err("composite thread panicked".into()));
    let _ = std::fs::remove_dir_all(&temp);
    if let Err(err) = write_result {
        return Err(err);
    }
    if !status.success() {
        let detail = stderr_tail(&stderr_text);
        return Err(if detail.is_empty() {
            format!("ffmpeg exited with {status}")
        } else {
            format!("ffmpeg failed: {detail}")
        });
    }
    if !output.is_file() {
        return Err("ffmpeg exited cleanly but wrote no file".into());
    }
    Ok(())
}

#[cfg(feature = "ffmpeg")]
fn encode_audio_only_blocking(
    script: &FfmpegScript,
    output: &Path,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    shared: &std::sync::Arc<std::sync::Mutex<ExportSnapshot>>,
) -> Result<(), String> {
    let mut command = std::process::Command::new("ffmpeg");
    command
        .arg("-y")
        .arg("-hide_banner")
        .arg("-nostats")
        .arg("-stats_period")
        .arg("0.25")
        .arg("-progress")
        .arg("pipe:1");
    for input in &script.inputs {
        command.arg("-i").arg(input);
    }
    if script.filter.is_empty() {
        return Err("no audio graph to export".into());
    }
    command
        .arg("-filter_complex")
        .arg(&script.filter)
        .arg("-map")
        .arg(format!("[{}]", script.audio_label))
        .args(&script.codec_args)
        .arg("-f")
        .arg(&script.container)
        .arg(output);
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            "ffmpeg was not found on PATH".to_string()
        } else {
            err.to_string()
        }
    })?;
    let stderr = child.stderr.take();
    let stderr_handle = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut pipe) = stderr {
            use std::io::Read;
            let _ = pipe.read_to_string(&mut text);
        }
        text
    });
    let cancel_flag = cancel.clone();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_flag = stop.clone();
    let pid = child.id();
    let killer = std::thread::spawn(move || {
        while !stop_flag.load(std::sync::atomic::Ordering::Relaxed) {
            if cancel_flag.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = std::process::Command::new("kill")
                    .args(["-TERM", &pid.to_string()])
                    .status();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
    });
    if let Some(stdout) = child.stdout.take() {
        use std::io::{BufRead, BufReader};
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = child.kill();
                break;
            }
            if let Some(secs) = progress_seconds(&line) {
                let fraction = if script.duration_secs > 0.0 {
                    (secs / script.duration_secs).clamp(0.0, 0.99) as f32
                } else {
                    0.0
                };
                if let Ok(mut slot) = shared.lock() {
                    slot.fraction = fraction;
                    slot.message = format!("Encoding audio {:.0}%", fraction * 100.0);
                }
            }
        }
    }
    let status = child.wait().map_err(|err| err.to_string())?;
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = killer.join();
    let stderr_text = stderr_handle.join().unwrap_or_default();
    if !status.success() {
        let detail = stderr_tail(&stderr_text);
        return Err(if detail.is_empty() {
            format!("ffmpeg exited with {status}")
        } else {
            format!("ffmpeg failed: {detail}")
        });
    }
    if !output.is_file() {
        return Err("ffmpeg exited cleanly but wrote no file".into());
    }
    Ok(())
}

#[cfg(feature = "ffmpeg")]
fn progress_seconds(line: &str) -> Option<f64> {
    let (key, value) = line.trim().split_once('=')?;
    match key {
        "out_time_us" | "out_time_ms" => {
            value.parse::<f64>().ok().map(|micros| micros / 1_000_000.0)
        }
        "out_time" => parse_clock(value),
        _ => None,
    }
}

#[cfg(feature = "ffmpeg")]
fn parse_clock(value: &str) -> Option<f64> {
    let mut parts = value.split(':');
    let hours: f64 = parts.next()?.parse().ok()?;
    let mins: f64 = parts.next()?.parse().ok()?;
    let secs: f64 = parts.next()?.parse().ok()?;
    Some(hours * 3600.0 + mins * 60.0 + secs)
}

#[cfg(feature = "ffmpeg")]
fn stderr_tail(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    lines
        .iter()
        .rev()
        .take(4)
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(feature = "ffmpeg")]
fn write_raster(
    stdin: &mut impl std::io::Write,
    plan: &RasterPlan,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    shared: &std::sync::Arc<std::sync::Mutex<ExportSnapshot>>,
) -> Result<(), String> {
    let mut cache = DecodeCache::default();
    let total = plan.frames.len().max(1) as f32;
    for (index, frame) in plan.frames.iter().enumerate() {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("export cancelled".into());
        }
        let rgba = render_frame(frame, plan, &mut cache)?;
        stdin.write_all(&rgba).map_err(|err| err.to_string())?;
        if let Ok(mut slot) = shared.lock() {
            let fraction = ((index as f32 + 1.0) / total).clamp(0.0, 0.99);
            slot.fraction = fraction;
            slot.message = format!("Encoding {:.0}%", fraction * 100.0);
        }
    }
    Ok(())
}

#[cfg(feature = "ffmpeg")]
fn render_frame(
    frame: &RasterFrame,
    plan: &RasterPlan,
    cache: &mut DecodeCache,
) -> Result<Vec<u8>, String> {
    let mut rgba = compose_layers(
        plan.width,
        plan.height,
        plan.seq_width,
        plan.seq_height,
        &frame.layers,
        |layer| cache.load(layer).map(|pixels| pixels.to_vec()),
    )?;
    crate::composite::burn_captions(&mut rgba, plan.width, plan.height, &frame.captions);
    Ok(rgba)
}

#[cfg(feature = "ffmpeg")]
#[derive(Default)]
struct DecodeCache {
    map: std::collections::HashMap<(String, u32, u32, i64), std::sync::Arc<[u8]>>,
    order: std::collections::VecDeque<(String, u32, u32, i64)>,
}

#[cfg(feature = "ffmpeg")]
impl DecodeCache {
    fn load(&mut self, layer: &ProgramLayer) -> Result<std::sync::Arc<[u8]>, String> {
        let LayerSource::Media {
            path,
            source_frame,
            time_secs,
            frame_secs: _,
            last_source_frame: _,
        } = &layer.source
        else {
            return Err(format!("{} is a title and is not decoded", layer.label));
        };
        let key = (path.clone(), layer.width, layer.height, *source_frame);
        if let Some(hit) = self.map.get(&key) {
            return Ok(hit.clone());
        }
        let count = 8.min(crate::MAX_BURST);
        let request = crate::FrameRequest::new(path, *time_secs, layer.width, layer.height, count)
            .map_err(|err| err.to_string())?;
        let decoded = crate::decode_frames(&request).map_err(|err| err.to_string())?;
        if decoded.is_empty() {
            return Err(format!("ffmpeg returned no frame for {path}"));
        }
        let mut first = None;
        for (offset, frame) in decoded.into_iter().enumerate() {
            let src = source_frame.saturating_add(offset as i64);
            let key = (path.clone(), layer.width, layer.height, src);
            let pixels: std::sync::Arc<[u8]> = std::sync::Arc::from(frame.rgba.into_boxed_slice());
            if offset == 0 {
                first = Some(pixels.clone());
            }
            self.insert(key, pixels);
        }
        first.ok_or_else(|| format!("ffmpeg returned no frame for {path}"))
    }

    fn insert(&mut self, key: (String, u32, u32, i64), pixels: std::sync::Arc<[u8]>) {
        if self.map.contains_key(&key) {
            return;
        }
        self.order.push_back(key.clone());
        self.map.insert(key, pixels);
        while self.order.len() > 24 {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }
}

#[derive(Default)]
struct Graph {
    inputs: Vec<String>,
    filters: Vec<String>,
    n: u32,
}

impl Graph {
    fn lab(&mut self) -> String {
        let id = self.n;
        self.n += 1;
        format!("n{id}")
    }

    fn add_input(&mut self, path: &str) -> usize {
        self.inputs.push(path.to_string());
        self.inputs.len() - 1
    }
}

fn slice_audio(
    graph: &mut Graph,
    pieces: &[WavPiece],
    start: i64,
    end: i64,
    timebase: Timebase,
    dur: f64,
    master: f32,
) -> String {
    let mut layers = Vec::new();
    for piece in pieces {
        if piece.timeline_out <= start || piece.timeline_in >= end {
            continue;
        }
        let overlap_in = piece.timeline_in.max(start);
        let overlap_out = piece.timeline_out.min(end);
        let src = piece.source_at_in
            + (overlap_in - piece.timeline_in) as f64 * piece.seconds_per_frame.max(0.0);
        let src_dur = (overlap_out - overlap_in).max(1) as f64 * piece.seconds_per_frame.max(0.0);
        let delay_ms = frames_secs(overlap_in - start, timebase).max(0.0) * 1000.0;
        let index = graph.add_input(&piece.path);
        let label = graph.lab();
        let curve = if piece.gain_keys.is_empty() {
            GainCurve::constant(piece.gain)
        } else {
            GainCurve {
                constant: piece.gain,
                keys: piece.gain_keys.clone(),
            }
        };
        let volume = ffmpeg_volume_arg(&curve, overlap_in, timebase.fps_f64());
        let mut filter = format!(
            "[{index}:a]atrim=start={}:end={},asetpts=PTS-STARTPTS,volume={volume}",
            secs(src),
            secs(src + src_dur.max(0.01)),
        );
        if let Some(pan) = ffmpeg_pan_filter(piece.pan) {
            filter.push(',');
            filter.push_str(&pan);
        }
        filter.push_str(&format!(
            ",adelay={}|{},aformat=sample_rates=48000:channel_layouts=stereo[{label}]",
            delay_ms.round() as i64,
            delay_ms.round() as i64,
        ));
        graph.filters.push(filter);
        layers.push(label);
    }
    let summed = if layers.is_empty() {
        let label = graph.lab();
        graph.filters.push(format!(
            "anullsrc=channel_layout=stereo:sample_rate=48000:d={},aformat=sample_fmts=fltp:channel_layouts=stereo[{label}]",
            secs(dur)
        ));
        label
    } else if layers.len() == 1 {
        layers.remove(0)
    } else {
        let label = graph.lab();
        let inputs: String = layers.iter().map(|label| format!("[{label}]")).collect();
        graph.filters.push(format!(
            "{inputs}amix=inputs={}:duration=longest:dropout_transition=0:normalize=0[{label}]",
            layers.len()
        ));
        label
    };
    let master = editor_core::clamp_gain(master);
    let mixed = if (master - 1.0).abs() > 1.0e-4 {
        let label = graph.lab();
        graph
            .filters
            .push(format!("[{summed}]volume={master:.4}[{label}]"));
        label
    } else {
        summed
    };
    let out = graph.lab();
    graph.filters.push(format!(
        "[{mixed}]apad,atrim=0:{},asetpts=PTS-STARTPTS,aformat=sample_rates=48000:channel_layouts=stereo[{out}]",
        secs(dur)
    ));
    out
}

fn audible_pieces(
    sequence: &Sequence,
    media: &[MediaAsset],
    from: i64,
    to: i64,
    warnings: &mut Vec<String>,
) -> Vec<WavPiece> {
    let mut pieces = Vec::new();
    for region in mix_regions(sequence, from, to, false) {
        let Some(asset) = media.iter().find(|item| item.id == region.media_id) else {
            continue;
        };
        if !asset.has_audio {
            continue;
        }
        let Some(clip) = sequence
            .tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .find(|clip| clip.id.0 == region.clip_id)
        else {
            continue;
        };
        let resolved = crate::resolve_media_path(&asset.path);
        if !resolved.is_file() {
            warnings.push(format!("Skipped offline audio {}", asset.name));
            continue;
        }
        let at_in = source_frame_at(clip, clip.timeline_in, sequence.timebase);
        let at_next = source_frame_at(clip, Frame(clip.timeline_in.0 + 1), sequence.timebase);
        let mut seconds_per_frame =
            (at_next.0 - at_in.0) as f64 * clip.media_timebase.frame_duration_secs();
        if seconds_per_frame <= 0.0 {
            seconds_per_frame = sequence.timebase.frame_duration_secs();
        }
        pieces.push(WavPiece {
            path: resolved.to_string_lossy().into_owned(),
            timeline_in: region.timeline_in,
            timeline_out: region.timeline_out,
            source_at_in: at_in.to_seconds(clip.media_timebase).max(0.0),
            seconds_per_frame,
            gain: region.gain,
            pan: region.pan,
            gain_keys: region.gain_keys,
        });
    }
    pieces
}

fn caption_cues(sequence: &Sequence, range_in: i64, range_out: i64) -> Vec<(f64, f64, String)> {
    let mut cues = Vec::new();
    for track in &sequence.tracks {
        if track.kind != TrackKind::Caption || track.muted {
            continue;
        }
        for cue in &track.cues {
            if cue.timeline_out.0 <= range_in || cue.timeline_in.0 >= range_out {
                continue;
            }
            let start = frames_secs(
                cue.timeline_in.0.max(range_in) - range_in,
                sequence.timebase,
            );
            let end = frames_secs(
                cue.timeline_out.0.min(range_out) - range_in,
                sequence.timebase,
            );
            if end > start && !cue.text.trim().is_empty() {
                cues.push((start, end, cue.text.clone()));
            }
        }
    }
    cues.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    cues
}

fn export_bounds(sequence: &Sequence, range: ExportRange) -> Result<(i64, i64), String> {
    let end = sequence.end_frame().0.max(0);
    let (inn, out) = match range {
        ExportRange::InOut => {
            let inn = sequence.in_point.unwrap_or(Frame::ZERO).0.max(0);
            let out = sequence
                .out_point
                .map(|frame| frame.0)
                .unwrap_or(end)
                .max(inn);
            (inn, out)
        }
        ExportRange::WholeSequence => (0, end),
    };
    if out <= inn {
        return Err("export range is empty".into());
    }
    Ok((inn, out))
}

fn frames_secs(frames: i64, timebase: Timebase) -> f64 {
    frames.max(0) as f64 * timebase.frame_duration_secs()
}

fn fps_token(timebase: Timebase) -> String {
    if timebase.denominator == 1 {
        timebase.numerator.to_string()
    } else {
        format!("{}/{}", timebase.numerator, timebase.denominator)
    }
}

fn even_dim(value: u32) -> u32 {
    (value.max(2) / 2) * 2
}

fn secs(value: f64) -> String {
    format!("{:.5}", value.max(0.0))
}

fn srt_time(secs: f64) -> String {
    let total = (secs.max(0.0) * 1000.0).round() as i64;
    let hours = total / 3_600_000;
    let mins = (total % 3_600_000) / 60_000;
    let whole = (total % 60_000) / 1000;
    let millis = total % 1000;
    format!("{hours:02}:{mins:02}:{whole:02},{millis:03}")
}

fn codec_args(codec: &str, container: &str, hints: &EncodeHints) -> Vec<String> {
    if hints.audio_only || is_audio_only_codec(codec, container) {
        return split_args("-c:a pcm_s16le -ar 48000");
    }
    let audio_kbps = hints.audio_bitrate_kbps.unwrap_or(192);
    let audio = format!("-b:a {audio_kbps}k");
    match codec.trim().to_ascii_lowercase().as_str() {
        "h.265" | "h265" | "hevc" => {
            if let Some(video_kbps) = hints.video_bitrate_kbps {
                split_args(&format!(
                    "-c:v libx265 -tag:v hvc1 -preset veryfast -b:v {video_kbps}k -maxrate {video_kbps}k -bufsize {}k -pix_fmt yuv420p -c:a aac {audio}",
                    video_kbps * 2
                ))
            } else {
                split_args(&format!(
                    "-c:v libx265 -tag:v hvc1 -preset veryfast -crf 20 -pix_fmt yuv420p -c:a aac {audio}"
                ))
            }
        }
        "prores 422" => split_args(&format!(
            "-c:v prores_ks -profile:v 2 -pix_fmt yuv422p10le -c:a aac {audio}"
        )),
        "prores 4444" => split_args(&format!(
            "-c:v prores_ks -profile:v 4 -pix_fmt yuva444p10le -c:a aac {audio}"
        )),
        "dnxhr hq" | "dnxhr" => split_args(&format!(
            "-c:v dnxhd -profile:v dnxhr_hq -pix_fmt yuv422p -c:a aac {audio}"
        )),
        _ => {
            if let Some(video_kbps) = hints.video_bitrate_kbps {
                split_args(&format!(
                    "-c:v libx264 -preset veryfast -b:v {video_kbps}k -maxrate {video_kbps}k -bufsize {}k -pix_fmt yuv420p -c:a aac {audio} -movflags +faststart",
                    video_kbps * 2
                ))
            } else {
                split_args(&format!(
                    "-c:v libx264 -preset veryfast -crf 18 -pix_fmt yuv420p -c:a aac {audio} -movflags +faststart"
                ))
            }
        }
    }
}

fn is_audio_only_codec(codec: &str, container: &str) -> bool {
    matches!(
        codec.trim().to_ascii_lowercase().as_str(),
        "pcm" | "wav" | "audio only" | "audio-only"
    ) || matches!(container.trim().to_ascii_lowercase().as_str(), "wav")
}

fn split_args(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

#[cfg(all(test, feature = "ffmpeg"))]
fn block_mae(rgba: &[u8], rgb: &[u8], width: u32, height: u32, block: u32) -> f32 {
    let bw = (width / block).max(1);
    let bh = (height / block).max(1);
    let mut err = 0.0f32;
    let mut count = 0.0f32;
    for by in 0..bh {
        for bx in 0..bw {
            let mut src = [0.0f32; 3];
            let mut dst = [0.0f32; 3];
            let mut n = 0.0f32;
            let x0 = bx * block;
            let y0 = by * block;
            for y in y0..(y0 + block).min(height) {
                for x in x0..(x0 + block).min(width) {
                    let rgba_i = (y as usize * width as usize + x as usize) * 4;
                    let rgb_i = (y as usize * width as usize + x as usize) * 3;
                    for channel in 0..3 {
                        src[channel] += rgba[rgba_i + channel] as f32;
                        dst[channel] += rgb[rgb_i + channel] as f32;
                    }
                    n += 1.0;
                }
            }
            for channel in 0..3 {
                err += (src[channel] / n - dst[channel] / n).abs();
                count += 1.0;
            }
        }
    }
    err / count.max(1.0)
}

fn container_name(container: &str) -> &'static str {
    match container.trim().to_ascii_lowercase().as_str() {
        "mov" => "mov",
        "mxf" => "mxf",
        "wav" => "wav",
        _ => "mp4",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::{
        Clip, ClipId, ColorGrade, CueId, Effect, MediaId, SequenceId, TrackId, Transform,
        Transition, TransitionAlign, TransitionId, TransitionKind,
    };

    fn touch(dir: &std::path::Path, name: &str) -> String {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, b"media").unwrap();
        path.to_string_lossy().into_owned()
    }

    fn asset(id: u64, path: &str, video: bool, audio: bool) -> MediaAsset {
        MediaAsset {
            id: MediaId(id),
            bin_id: editor_core::BinId(1),
            name: path.into(),
            path: path.into(),
            duration: Frame(240),
            timebase: Timebase::fps_24(),
            width: Some(320),
            height: Some(180),
            video_codec: video.then(|| "h264".into()),
            audio_codec: audio.then(|| "aac".into()),
            audio_channels: audio.then_some(2),
            sample_rate: audio.then_some(48_000),
            has_video: video,
            has_audio: audio,
            offline: false,
            proxy_path: None,
        }
    }

    #[test]
    fn graph_bakes_grade_overlay_transition_gain_and_captions() {
        let dir = std::env::temp_dir().join(format!("meridian-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let picture = touch(&dir, "picture.mp4");
        let pip = touch(&dir, "pip.mp4");
        let voice = touch(&dir, "voice.wav");
        let mut sequence = Sequence::new(SequenceId(1), "Cut", 320, 180, Timebase::fps_24());
        sequence.add_track(TrackId(2), TrackKind::Video, "V1");
        sequence.add_track(TrackId(3), TrackKind::Video, "V2");
        sequence.add_track(TrackId(4), TrackKind::Audio, "A1");
        sequence.add_track(TrackId(5), TrackKind::Caption, "C1");
        let mut left = Clip::basic(10, 0, 48);
        left.media_id = Some(MediaId(1));
        left.name = "LEFT".into();
        let mut grade = ColorGrade::neutral();
        grade.exposure.base = 0.5;
        left.effects.push(Effect::Color(grade));
        let mut right = Clip::basic(11, 48, 96);
        right.media_id = Some(MediaId(1));
        right.name = "RIGHT".into();
        let mut inset = Clip::basic(12, 12, 36);
        inset.media_id = Some(MediaId(2));
        let mut xform = Transform::identity();
        xform.scale_x.base = 0.4;
        xform.scale_y.base = 0.4;
        xform.position_x.base = 40.0;
        xform.position_y.base = -20.0;
        inset.effects.push(Effect::Transform(xform));
        sequence.tracks[0].clips = vec![left, right];
        sequence.tracks[0].transitions.push(Transition {
            id: TransitionId(7),
            kind: TransitionKind::CrossDissolve,
            left_clip: ClipId(10),
            right_clip: ClipId(11),
            duration: 8,
            alignment: TransitionAlign::Center,
        });
        sequence.tracks[1].clips = vec![inset];
        let mut audio = Clip::basic(20, 0, 96);
        audio.media_id = Some(MediaId(3));
        audio.volume.base = 0.25;
        sequence.tracks[2].clips = vec![audio];
        sequence.tracks[3].cues.push(editor_core::CaptionCue {
            id: CueId(30),
            timeline_in: Frame(0),
            timeline_out: Frame(24),
            text: "Hello, ridge".into(),
            speaker: None,
        });
        sequence.in_point = Some(Frame(12));
        sequence.out_point = Some(Frame(60));
        let media = vec![
            asset(1, &picture, true, true),
            asset(2, &pip, true, false),
            asset(3, &voice, false, true),
        ];
        let script =
            plan_encode(
                &sequence,
                &media,
                ExportRange::InOut,
                "H.264",
                "mp4",
                true,
                EncodeHints::default(),
            )
            .unwrap();
        let pip_frame = &script.raster.frames[8];
        assert!(
            pip_frame
                .layers
                .iter()
                .any(|layer| (layer.grade.exposure - 0.5).abs() < 1.0e-4),
            "graded base missing"
        );
        let pip = pip_frame
            .layers
            .iter()
            .find(|layer| (layer.place.scale_x - 0.4).abs() < 1.0e-3)
            .expect("picture-in-picture layer");
        assert!((pip.place.pos_x - 40.0).abs() < 1.0e-3);
        assert!((pip.place.pos_y - (-20.0)).abs() < 1.0e-3);
        assert!((pip.place.anchor_x - 0.5).abs() < 1.0e-3);
        assert!(
            pip_frame.layers.len() >= 2,
            "base should sit under the inset"
        );
        let dissolve = &script.raster.frames[36];
        assert!(dissolve.layers.len() >= 2, "dissolve needs both sides");
        assert!((dissolve.layers[0].place.opacity - 1.0).abs() < 1.0e-3);
        assert!((dissolve.layers[1].place.opacity - 0.5).abs() < 1.0e-3);
        assert!(script.raster.frames[0]
            .captions
            .iter()
            .any(|line| line.contains("Hello, ridge")));
        assert!(script.filter.contains("volume=0.2500"));
        assert!(script.srt.contains("Hello, ridge"));
        assert!(script.codec_args.iter().any(|arg| arg == "libx264"));
        assert!((script.duration_secs - 2.0).abs() < 1.0e-6);
        assert!(script.inputs.iter().any(|path| path.ends_with("voice.wav")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn muted_picture_exports_black_and_solo_drops_other_audio() {
        let dir = std::env::temp_dir().join(format!("meridian-plan-mute-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let picture = touch(&dir, "picture.mp4");
        let keep = touch(&dir, "keep.wav");
        let drop = touch(&dir, "drop.wav");
        let mut sequence = Sequence::new(SequenceId(1), "Cut", 320, 180, Timebase::fps_24());
        sequence.add_track(TrackId(2), TrackKind::Video, "V1");
        sequence.add_track(TrackId(3), TrackKind::Audio, "A1");
        sequence.add_track(TrackId(4), TrackKind::Audio, "A2");
        let mut clip = Clip::basic(10, 0, 24);
        clip.media_id = Some(MediaId(1));
        sequence.tracks[0].clips = vec![clip];
        sequence.tracks[0].muted = true;
        let mut loud = Clip::basic(11, 0, 24);
        loud.media_id = Some(MediaId(2));
        let mut quiet = Clip::basic(12, 0, 24);
        quiet.media_id = Some(MediaId(3));
        sequence.tracks[1].clips = vec![loud];
        sequence.tracks[1].solo = true;
        sequence.tracks[2].clips = vec![quiet];
        let media = vec![
            asset(1, &picture, true, false),
            asset(2, &keep, false, true),
            asset(3, &drop, false, true),
        ];
        let script = plan_encode(
            &sequence,
            &media,
            ExportRange::WholeSequence,
            "H.264",
            "mp4",
            false,
            EncodeHints::default(),
        )
        .unwrap();
        assert!(script
            .raster
            .frames
            .iter()
            .all(|frame| frame.layers.is_empty()));
        assert!(script
            .raster
            .frames
            .iter()
            .all(|frame| frame.captions.is_empty()));
        assert!(!script
            .inputs
            .iter()
            .any(|path| path.ends_with("picture.mp4")));
        assert!(script.inputs.iter().any(|path| path.ends_with("keep.wav")));
        assert!(!script.inputs.iter().any(|path| path.ends_with("drop.wav")));
        assert!(script.srt.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deliver_preset_hints_shape_codec_args() {
        let dir = std::env::temp_dir().join(format!("meridian-plan-hints-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let voice = touch(&dir, "voice.wav");
        let mut sequence = Sequence::new(SequenceId(1), "Hints", 320, 180, Timebase::fps_24());
        sequence.add_track(TrackId(2), TrackKind::Audio, "A1");
        let mut audio = Clip::basic(10, 0, 24);
        audio.media_id = Some(MediaId(1));
        sequence.tracks[0].clips = vec![audio];
        let media = vec![asset(1, &voice, false, true)];
        let hints = EncodeHints {
            audio_bitrate_kbps: Some(256),
            video_bitrate_kbps: Some(8000),
            audio_only: false,
        };
        let script = plan_encode(
            &sequence,
            &media,
            ExportRange::WholeSequence,
            "H.264",
            "mp4",
            false,
            hints,
        )
        .unwrap();
        let joined = script.codec_args.join(" ");
        assert!(joined.contains("-b:v 8000k"));
        assert!(joined.contains("-b:a 256k"));

        let audio_only = plan_encode(
            &sequence,
            &media,
            ExportRange::WholeSequence,
            "PCM",
            "wav",
            false,
            EncodeHints {
                audio_only: true,
                ..EncodeHints::default()
            },
        )
        .unwrap();
        assert!(audio_only.audio_only);
        assert!(audio_only.raster.frames.is_empty());
        assert!(audio_only.codec_args.iter().any(|arg| arg == "pcm_s16le"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fader_pan_master_and_keyframes_share_the_playback_mix() {
        let dir = std::env::temp_dir().join(format!("meridian-plan-mix-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let voice = touch(&dir, "voice.wav");
        let mut sequence = Sequence::new(SequenceId(1), "Mix", 320, 180, Timebase::fps_24());
        sequence.add_track(TrackId(2), TrackKind::Video, "V1");
        sequence.add_track(TrackId(3), TrackKind::Audio, "A1");
        let mut picture = Clip::basic(10, 0, 24);
        picture.media_id = Some(MediaId(1));
        sequence.tracks[0].clips = vec![picture];
        let mut audio = Clip::basic(11, 0, 24);
        audio.media_id = Some(MediaId(2));
        audio.volume.base = 0.5;
        audio.volume.set_key(0, 0.5);
        audio.volume.set_key(12, 1.0);
        sequence.tracks[1].clips = vec![audio];
        sequence.tracks[1].fader = 0.5;
        sequence.tracks[1].pan = -1.0;
        sequence.master_fader = 0.5;
        let media = vec![asset(1, &voice, true, false), asset(2, &voice, false, true)];
        let script = plan_encode(
            &sequence,
            &media,
            ExportRange::WholeSequence,
            "H.264",
            "mp4",
            false,
            EncodeHints::default(),
        )
        .unwrap();
        assert!(
            script.filter.contains("eval=frame"),
            "keyed gain missing: {}",
            script.filter
        );
        assert!(
            script.filter.contains("0.2500"),
            "fader should scale the first key: {}",
            script.filter
        );
        assert!(
            script.filter.contains("pan=stereo|c0=1.4142*c0"),
            "hard left pan missing: {}",
            script.filter
        );
        assert!(
            script.filter.contains("volume=0.5000"),
            "master fader missing: {}",
            script.filter
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn srt_timestamps_are_relative_to_the_range() {
        let text = cues_to_srt(&[(0.5, 1.25, "Ridge".into())]);
        assert!(text.contains("00:00:00,500 --> 00:00:01,250"));
        assert!(text.contains("Ridge"));
    }

    #[cfg(feature = "ffmpeg")]
    #[test]
    fn encodes_an_h264_mp4_when_ffmpeg_is_available() {
        if crate::preview_backend() != crate::PreviewBackend::Cli {
            return;
        }
        let dir = std::env::temp_dir().join(format!("meridian-encode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let clip_path = dir.join("bars.mp4");
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=24:duration=2",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=2",
                "-shortest",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
            ])
            .arg(&clip_path)
            .status()
            .unwrap();
        assert!(status.success());
        let mut sequence = Sequence::new(SequenceId(1), "Bars", 160, 90, Timebase::fps_24());
        sequence.add_track(TrackId(2), TrackKind::Video, "V1");
        sequence.add_track(TrackId(3), TrackKind::Audio, "A1");
        sequence.add_track(TrackId(4), TrackKind::Caption, "C1");
        let mut video = Clip::basic(10, 0, 24);
        video.media_id = Some(MediaId(1));
        let mut grade = ColorGrade::neutral();
        grade.exposure.base = 0.4;
        video.effects.push(Effect::Color(grade));
        let mut audio = Clip::basic(11, 0, 24);
        audio.media_id = Some(MediaId(1));
        audio.volume.base = 0.5;
        sequence.tracks[0].clips = vec![video];
        sequence.tracks[1].clips = vec![audio];
        sequence.tracks[2].cues.push(editor_core::CaptionCue {
            id: CueId(4),
            timeline_in: Frame(0),
            timeline_out: Frame(24),
            text: "Exported".into(),
            speaker: None,
        });
        let path = clip_path.to_string_lossy().into_owned();
        let media = vec![asset(1, &path, true, true)];
        let script = plan_encode(
            &sequence,
            &media,
            ExportRange::WholeSequence,
            "H.264",
            "mp4",
            true,
            EncodeHints::default(),
        )
        .unwrap();
        let output = dir.join("out.mp4");
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shared = std::sync::Arc::new(std::sync::Mutex::new(ExportSnapshot {
            fraction: 0.0,
            message: String::new(),
            finished: false,
            ok: false,
        }));
        encode_blocking(&script, &output, &cancel, &shared).unwrap();
        assert!(output.is_file());
        assert!(std::fs::metadata(&output).unwrap().len() > 1000);
        let probe = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration:stream=codec_name,codec_type",
                "-of",
                "default=nw=1",
            ])
            .arg(&output)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&probe.stdout);
        assert!(probe.status.success(), "{text}");
        assert!(text.contains("codec_name=h264"), "{text}");
        assert!(text.contains("codec_name=aac"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "ffmpeg")]
    #[test]
    fn demo_sequence_opening_is_not_a_black_frame() {
        if crate::preview_backend() != crate::PreviewBackend::Cli {
            return;
        }
        let mut project = editor_core::demo_project();
        let sequence = project.active_mut().unwrap();
        sequence.in_point = Some(Frame(48));
        sequence.out_point = Some(Frame(72));
        let sequence = project.active().unwrap();
        let script = plan_encode(
            sequence,
            &project.media,
            ExportRange::InOut,
            "H.264",
            "mp4",
            true,
            EncodeHints::default(),
        )
        .unwrap();
        let dir = std::env::temp_dir().join(format!("meridian-demo-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let output = dir.join("demo.mp4");
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shared = std::sync::Arc::new(std::sync::Mutex::new(ExportSnapshot {
            fraction: 0.0,
            message: String::new(),
            finished: false,
            ok: false,
        }));
        encode_blocking(&script, &output, &cancel, &shared).unwrap_or_else(|err| {
            panic!("{err}\n{}", script.filter);
        });
        let raw = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-ss", "0.4", "-i"])
            .arg(&output)
            .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .output()
            .unwrap();
        assert!(raw.status.success());
        let pixels = &raw.stdout;
        let mut bright = 0usize;
        for chunk in pixels.chunks(3) {
            if chunk.len() == 3 && (chunk[0] > 20 || chunk[1] > 20 || chunk[2] > 20) {
                bright += 1;
            }
        }
        let frac = bright as f32 / (pixels.len() as f32 / 3.0);
        assert!(
            frac > 0.4,
            "opening frame is mostly black ({frac:.3}); filter starts with {}",
            &script.filter.chars().take(400).collect::<String>()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "ffmpeg")]
    #[test]
    fn exported_pip_frame_stays_close_to_the_shared_composite() {
        if crate::preview_backend() != crate::PreviewBackend::Cli {
            return;
        }
        let mut project = editor_core::demo_project();
        let sequence = project.active_mut().unwrap();
        sequence.in_point = Some(Frame(60));
        sequence.out_point = Some(Frame(61));
        let sequence = project.active().unwrap();
        let script = plan_encode(
            sequence,
            &project.media,
            ExportRange::InOut,
            "H.264",
            "mp4",
            true,
            EncodeHints::default(),
        )
        .unwrap();
        assert_eq!(script.raster.frames.len(), 1);
        assert_eq!(script.raster.frames[0].layers.len(), 3);
        assert!(script.raster.frames[0].layers[2].is_title());
        assert!(script.raster.frames[0]
            .captions
            .iter()
            .any(|line| line.contains("ridge")));
        let mut cache = DecodeCache::default();
        let rgba = render_frame(&script.raster.frames[0], &script.raster, &mut cache).unwrap();
        let dir = std::env::temp_dir().join(format!("meridian-match-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let composite_png = dir.join("composite.png");
        let raw = dir.join("composite.rgba");
        std::fs::write(&raw, &rgba).unwrap();
        let dumped = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-s",
            ])
            .arg(format!("{}x{}", script.raster.width, script.raster.height))
            .arg("-i")
            .arg(&raw)
            .args(["-frames:v", "1"])
            .arg(&composite_png)
            .status()
            .unwrap();
        assert!(dumped.success());
        let output = dir.join("out.mp4");
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shared = std::sync::Arc::new(std::sync::Mutex::new(ExportSnapshot {
            fraction: 0.0,
            message: String::new(),
            finished: false,
            ok: false,
        }));
        encode_blocking(&script, &output, &cancel, &shared).unwrap();
        let encoded = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&output)
            .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .output()
            .unwrap();
        assert!(encoded.status.success());
        let rgb = &encoded.stdout;
        assert_eq!(
            rgb.len(),
            script.raster.width as usize * script.raster.height as usize * 3
        );
        let mae = block_mae(&rgba, rgb, script.raster.width, script.raster.height, 16);
        if std::env::var_os("MERIDIAN_PROOF").is_some() {
            let _ = std::fs::copy(&composite_png, "/tmp/meridian-composite.png");
            let _ = std::fs::copy(&output, "/tmp/meridian-pip.mp4");
            let export_png = dir.join("export.png");
            let _ = std::process::Command::new("ffmpeg")
                .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
                .arg(&output)
                .args(["-frames:v", "1"])
                .arg(&export_png)
                .status();
            let _ = std::fs::copy(&export_png, "/tmp/meridian-export.png");
            for (frame, name) in [(144i64, "dissolve"), (264, "wipe")] {
                let mut project = editor_core::demo_project();
                let sequence = project.active_mut().unwrap();
                sequence.in_point = Some(Frame(frame));
                sequence.out_point = Some(Frame(frame + 1));
                let sequence = project.active().unwrap();
                let script = plan_encode(
                    sequence,
                    &project.media,
                    ExportRange::InOut,
                    "H.264",
                    "mp4",
                    true,
                )
                .unwrap();
                let mut cache = DecodeCache::default();
                let rgba =
                    render_frame(&script.raster.frames[0], &script.raster, &mut cache).unwrap();
                let raw = dir.join(format!("{name}.rgba"));
                std::fs::write(&raw, &rgba).unwrap();
                let png = dir.join(format!("{name}.png"));
                let _ = std::process::Command::new("ffmpeg")
                    .args([
                        "-y",
                        "-hide_banner",
                        "-loglevel",
                        "error",
                        "-f",
                        "rawvideo",
                        "-pix_fmt",
                        "rgba",
                        "-s",
                    ])
                    .arg(format!("{}x{}", script.raster.width, script.raster.height))
                    .arg("-i")
                    .arg(&raw)
                    .args(["-frames:v", "1"])
                    .arg(&png)
                    .status();
                let _ = std::fs::copy(&png, format!("/tmp/meridian-{name}.png"));
            }
            eprintln!("block mae {mae:.3}");
        }
        assert!(
            mae < 8.0,
            "export drifted from the shared composite (16px block mae {mae:.2})"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(all(feature = "ffmpeg", feature = "whisper"))]
    #[test]
    fn whisper_words_become_burned_captions_on_a_graded_export() {
        if crate::preview_backend() != crate::PreviewBackend::Cli {
            return;
        }
        if crate::whisper_availability().is_err() {
            return;
        }
        if std::process::Command::new("espeak-ng")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let dir =
            std::env::temp_dir().join(format!("meridian-whisper-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("voice.wav");
        let spoken = "We came in over the ridge at first light.";
        let status = std::process::Command::new("espeak-ng")
            .args(["-s", "130", "-w"])
            .arg(&wav)
            .arg(spoken)
            .status()
            .unwrap();
        assert!(status.success());
        let clip_path = dir.join("speech.mp4");
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=24:duration=6",
                "-i",
            ])
            .arg(&wav)
            .args([
                "-shortest",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
            ])
            .arg(&clip_path)
            .status()
            .unwrap();
        assert!(status.success());

        let mut sequence = Sequence::new(SequenceId(1), "Speech", 320, 180, Timebase::fps_24());
        sequence.add_track(TrackId(2), TrackKind::Video, "V1");
        sequence.add_track(TrackId(3), TrackKind::Audio, "A1");
        sequence.add_track(TrackId(4), TrackKind::Caption, "C1");
        let mut video = Clip::basic(10, 0, 96);
        video.media_id = Some(MediaId(1));
        let mut grade = ColorGrade::neutral();
        grade.exposure.base = 0.45;
        video.effects.push(Effect::Color(grade));
        let mut audio = Clip::basic(11, 0, 96);
        audio.media_id = Some(MediaId(1));
        audio.volume.base = 0.8;
        sequence.tracks[0].clips = vec![video];
        sequence.tracks[1].clips = vec![audio];
        let path = clip_path.to_string_lossy().into_owned();
        let media = vec![asset(1, &path, true, true)];
        let pieces = audible_pieces(&sequence, &media, 0, 96, &mut Vec::new());
        assert_eq!(pieces.len(), 1);
        let extracted = dir.join("caption.wav");
        write_timeline_wav(&pieces, 0, 96, sequence.timebase, &extracted).unwrap();
        let words = crate::transcribe_wav(&extracted, Some("en")).unwrap();
        let drafts = editor_core::map_words_to_cues(
            &words,
            Frame(0),
            Frame(96),
            sequence.timebase,
            Some("Voice".into()),
        );
        assert!(!drafts.is_empty(), "whisper returned no cues");
        let joined = drafts
            .iter()
            .map(|cue| cue.text.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            joined.contains("ridge") || joined.contains("came"),
            "{joined}"
        );
        for (index, draft) in drafts.iter().enumerate() {
            sequence.tracks[2].cues.push(editor_core::CaptionCue {
                id: CueId(100 + index as u64),
                timeline_in: draft.timeline_in,
                timeline_out: draft.timeline_out,
                text: draft.text.clone(),
                speaker: draft.speaker.clone(),
            });
        }
        let script = plan_encode(
            &sequence,
            &media,
            ExportRange::WholeSequence,
            "H.264",
            "mp4",
            true,
            EncodeHints::default(),
        )
        .unwrap();
        assert!(script
            .raster
            .frames
            .iter()
            .any(|frame| frame
                .layers
                .iter()
                .any(|layer| (layer.grade.exposure - 0.45).abs() < 1.0e-4)));
        assert!(script
            .raster
            .frames
            .iter()
            .any(|frame| !frame.captions.is_empty()));
        assert!(script.filter.contains("volume=0.8000"));
        let output = dir.join("out.mp4");
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shared = std::sync::Arc::new(std::sync::Mutex::new(ExportSnapshot {
            fraction: 0.0,
            message: String::new(),
            finished: false,
            ok: false,
        }));
        encode_blocking(&script, &output, &cancel, &shared).unwrap();
        let frame = dir.join("frame.png");
        let dumped = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-ss",
                "1.0",
                "-i",
            ])
            .arg(&output)
            .args(["-frames:v", "1"])
            .arg(&frame)
            .status()
            .unwrap();
        assert!(dumped.success());
        let raw = dir.join("frame.rgb");
        let dumped = std::process::Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(&frame)
            .args(["-f", "rawvideo", "-pix_fmt", "rgb24"])
            .arg(&raw)
            .status()
            .unwrap();
        assert!(dumped.success());
        let pixels = std::fs::read(&raw).unwrap();
        assert_eq!(pixels.len(), 320 * 180 * 3);
        let stats = |y0: usize, y1: usize| -> (f32, f32) {
            let mut sum = 0u64;
            let mut sq = 0u64;
            let mut count = 0u64;
            for y in y0..y1 {
                for x in 20..300 {
                    let i = (y * 320 + x) * 3;
                    for channel in 0..3 {
                        let value = pixels[i + channel] as u64;
                        sum += value;
                        sq += value * value;
                        count += 1;
                    }
                }
            }
            let mean = sum as f32 / count as f32;
            let var = (sq as f32 / count as f32) - mean * mean;
            (mean, var.max(0.0).sqrt())
        };
        let style = crate::caption_style(180);
        let band_bottom = 180 - style.margin as usize;
        let band_top = band_bottom.saturating_sub(style.glyph as usize + 8);
        let (picture, _) = stats(20, 70);
        let (burned, burned_dev) = stats(band_top, band_bottom);
        if std::env::var_os("MERIDIAN_KEEP_EXPORT").is_some() {
            let _ = std::fs::copy(&output, "/tmp/meridian-whisper-export.mp4");
            let _ = std::fs::copy(&frame, "/tmp/meridian-whisper-frame.png");
        }
        assert!(
            picture > 40.0,
            "graded picture is unexpectedly dark: {picture}"
        );
        assert!(
            burned_dev > 35.0,
            "caption band is flat (mean {burned}, stddev {burned_dev}); cues: {joined}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
