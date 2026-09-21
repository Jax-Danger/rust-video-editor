//! Timeline export.
//!
//! [`plan_encode`] builds an ffmpeg filter graph for the sequence (or its
//! in/out). It is pure and does not spawn anything, so `cargo test` can check
//! grades, overlays, transitions, and captions without the `ffmpeg` feature.
//! With that feature, [`spawn_export`] runs the graph and reports progress.

use std::path::{Path, PathBuf};

use editor_core::{
    color_grade, source_frame_at, transform, AnimatedF32, Clip, ColorGrade, Effect, ExportRange,
    Frame, MediaAsset, Sequence, Timebase, Track, TrackKind, Transform, TransitionKind,
};

/// One audible region. Times are sequence frames; `source_at_in` is seconds
/// into the media at `timeline_in`.
#[derive(Clone, Debug)]
pub struct WavPiece {
    pub path: String,
    pub timeline_in: i64,
    pub timeline_out: i64,
    pub source_at_in: f64,
    pub seconds_per_frame: f64,
    pub gain: f32,
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
) -> Result<FfmpegScript, String> {
    let (range_in, range_out) = export_bounds(sequence, range)?;
    let mut warnings = Vec::new();
    let video_bounds = video_boundaries(sequence, range_in, range_out);
    let mut slices = pairs(&video_bounds);
    slices = subdivide_animated(sequence, slices);
    if slices.is_empty() {
        return Err("nothing to export".into());
    }

    let mut graph = Graph::default();
    let mut video_labels = Vec::new();
    let mut audio_labels = Vec::new();
    let width = even_dim(sequence.width);
    let height = even_dim(sequence.height);
    let fps = fps_token(sequence.timebase);
    let pieces = audible_pieces(sequence, media, range_in, range_out, &mut warnings);

    for (start, end) in slices {
        let dur = frames_secs(end - start, sequence.timebase);
        if dur <= 0.0 {
            continue;
        }
        let video = slice_video(
            &mut graph,
            sequence,
            media,
            start,
            end,
            width,
            height,
            &fps,
            dur,
        )?;
        let audio = slice_audio(&mut graph, &pieces, start, end, sequence.timebase, dur);
        video_labels.push(video);
        audio_labels.push(audio);
    }
    if video_labels.is_empty() {
        return Err("nothing to export".into());
    }

    let (cat_v, cat_a) = if video_labels.len() == 1 {
        (video_labels.remove(0), audio_labels.remove(0))
    } else {
        let cat_v = graph.lab();
        let cat_a = graph.lab();
        let mut chain = String::new();
        for (video, audio) in video_labels.iter().zip(audio_labels.iter()) {
            chain.push_str(&format!("[{video}][{audio}]"));
        }
        graph.filters.push(format!(
            "{chain}concat=n={}:v=1:a=1[{cat_v}][{cat_a}]",
            video_labels.len()
        ));
        (cat_v, cat_a)
    };

    let cues = caption_cues(sequence, range_in, range_out);
    let srt = cues_to_srt(&cues);
    let video_label = if burn_captions && !cues.is_empty() {
        match caption_font() {
            Some(font) => burn_drawtext(&mut graph, &cat_v, &cues, &font),
            None => {
                warnings.push(
                    "No DejaVu, Liberation, or Arial font found, so captions are soft subtitles only."
                        .into(),
                );
                cat_v
            }
        }
    } else {
        cat_v
    };

    Ok(FfmpegScript {
        inputs: graph.inputs,
        filter: graph.filters.join(";"),
        video_label,
        audio_label: cat_a,
        srt,
        duration_secs: frames_secs(range_out - range_in, sequence.timebase),
        codec_args: codec_args(codec),
        container: container_name(container).to_string(),
        warnings,
        burn_captions,
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
    let audio = slice_audio(&mut graph, pieces, range_in, range_out, timebase, dur);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let filter = graph.filters.join(";");
    let mut command = std::process::Command::new("ffmpeg");
    command.arg("-y").arg("-hide_banner").arg("-loglevel").arg("error").arg("-nostdin");
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
    let soft = !script.srt.trim().is_empty()
        && matches!(script.container.as_str(), "mp4" | "mov");
    if soft || (script.burn_captions && !script.srt.trim().is_empty()) {
        std::fs::write(&srt_path, &script.srt).map_err(|err| err.to_string())?;
    }

    let mut command = std::process::Command::new("ffmpeg");
    command
        .arg("-y")
        .arg("-hide_banner")
        .arg("-nostdin")
        .arg("-nostats")
        .arg("-stats_period")
        .arg("0.25")
        .arg("-progress")
        .arg("pipe:1");
    for input in &script.inputs {
        command.arg("-i").arg(input);
    }
    let srt_index = script.inputs.len();
    if soft {
        command.arg("-i").arg(&srt_path);
    }
    command
        .arg("-filter_complex")
        .arg(&script.filter)
        .arg("-map")
        .arg(format!("[{}]", script.video_label))
        .arg("-map")
        .arg(format!("[{}]", script.audio_label));
    if soft {
        command.arg("-map").arg(format!("{srt_index}:0"));
    }
    command.args(&script.codec_args);
    if soft {
        command.args(["-c:s", "mov_text", "-metadata:s:s:0", "language=eng"]);
    }
    command.arg("-f").arg(&script.container).arg(output);
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
                    slot.message = format!("Encoding {:.0}%", fraction * 100.0);
                }
            }
        }
    }
    let status = child.wait().map_err(|err| err.to_string())?;
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = killer.join();
    let stderr_text = stderr_handle.join().unwrap_or_default();
    let _ = std::fs::remove_dir_all(&temp);
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
        "out_time_us" | "out_time_ms" => value.parse::<f64>().ok().map(|micros| micros / 1_000_000.0),
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

fn slice_video(
    graph: &mut Graph,
    sequence: &Sequence,
    media: &[MediaAsset],
    start: i64,
    end: i64,
    width: u32,
    height: u32,
    fps: &str,
    dur: f64,
) -> Result<String, String> {
    let mut base = graph.lab();
    graph.filters.push(format!(
        "color=c=black:s={width}x{height}:r={fps}:d={},format=yuv420p,setsar=1[{base}]",
        secs(dur)
    ));
    for track in sequence
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video && track_visible(track, &sequence.tracks))
    {
        if let Some(blend) = transition_on(track, start, end) {
            let left = track
                .clips
                .iter()
                .find(|clip| clip.id == blend.left)
                .ok_or_else(|| "transition is missing its outgoing clip".to_string())?;
            let right = track
                .clips
                .iter()
                .find(|clip| clip.id == blend.right)
                .ok_or_else(|| "transition is missing its incoming clip".to_string())?;
            if blend.exact && end - start > 1 {
                let left_v = emit_picture(
                    graph,
                    sequence,
                    media,
                    left,
                    start,
                    end,
                    width,
                    height,
                    fps,
                    dur,
                    1.0,
                    start,
                )?;
                let right_v = emit_picture(
                    graph,
                    sequence,
                    media,
                    right,
                    start,
                    end,
                    width,
                    height,
                    fps,
                    dur,
                    1.0,
                    end - 1,
                )?;
                let mixed = graph.lab();
                let fade = (dur - frames_secs(1, sequence.timebase)).max(dur * 0.5);
                graph.filters.push(format!(
                    "[{left_v}][{right_v}]xfade=transition={}:duration={}:offset=0[{mixed}]",
                    xfade_name(&blend.kind),
                    secs(fade)
                ));
                let next = graph.lab();
                graph.filters.push(format!(
                    "[{base}][{mixed}]overlay=0:0:format=auto:eof_action=pass:shortest=1,format=yuv420p,setsar=1[{next}]"
                ));
                base = next;
            } else {
                let mid = start + (end - start) / 2;
                let progress = blend.progress_at(mid);
                let left_v = emit_picture(
                    graph, sequence, media, left, start, end, width, height, fps, dur,
                    1.0 - progress, mid,
                )?;
                let right_v = emit_picture(
                    graph, sequence, media, right, start, end, width, height, fps, dur, progress, mid,
                )?;
                for layer in [left_v, right_v] {
                    let next = graph.lab();
                    graph.filters.push(format!(
                        "[{base}][{layer}]overlay=0:0:format=auto:eof_action=pass:shortest=1,format=yuv420p,setsar=1[{next}]"
                    ));
                    base = next;
                }
            }
        } else if let Some(clip) = track
            .clips
            .iter()
            .find(|clip| clip.enabled && clip.timeline_in.0 < end && clip.timeline_out.0 > start)
        {
            let layer = emit_picture(
                graph, sequence, media, clip, start, end, width, height, fps, dur, 1.0, start,
            )?;
            let next = graph.lab();
            graph.filters.push(format!(
                "[{base}][{layer}]overlay=0:0:format=auto:eof_action=pass:shortest=1,format=yuv420p,setsar=1[{next}]"
            ));
            base = next;
        }
    }
    Ok(base)
}

struct Blend {
    left: editor_core::ClipId,
    right: editor_core::ClipId,
    kind: TransitionKind,
    start: i64,
    end: i64,
    exact: bool,
}

impl Blend {
    fn progress_at(&self, frame: i64) -> f32 {
        let span = (self.end - self.start).max(1) as f32;
        ((frame - self.start) as f32 / span).clamp(0.0, 1.0)
    }
}

fn transition_on(track: &Track, start: i64, end: i64) -> Option<Blend> {
    for transition in &track.transitions {
        let Some(left) = track.clips.iter().find(|clip| clip.id == transition.left_clip) else {
            continue;
        };
        let (t0, t1) = transition.range(left.timeline_out);
        if start >= t0.0 && end <= t1.0 && t1.0 > t0.0 {
            return Some(Blend {
                left: transition.left_clip,
                right: transition.right_clip,
                kind: transition.kind.clone(),
                start: t0.0,
                end: t1.0,
                exact: start == t0.0 && end == t1.0,
            });
        }
    }
    None
}

fn xfade_name(kind: &TransitionKind) -> &'static str {
    match kind {
        TransitionKind::CrossDissolve => "fade",
        TransitionKind::Wipe { .. } => "wipeleft",
        TransitionKind::PushSlide { .. } => "slideleft",
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_picture(
    graph: &mut Graph,
    sequence: &Sequence,
    media: &[MediaAsset],
    clip: &Clip,
    start: i64,
    end: i64,
    width: u32,
    height: u32,
    fps: &str,
    dur: f64,
    opacity_scale: f32,
    eval_frame: i64,
) -> Result<String, String> {
    let asset = clip
        .media_id
        .and_then(|id| media.iter().find(|item| item.id == id))
        .ok_or_else(|| format!("{} has no media", clip.name))?;
    if !asset.has_video {
        return Err(format!("{} has no picture", clip.name));
    }
    let resolved = crate::resolve_media_path(&asset.path);
    if !resolved.is_file() {
        return Err(format!("{} is offline ({})", clip.name, asset.path));
    }
    let src_start_frame = source_frame_at(clip, Frame(start), sequence.timebase);
    let src_end_frame = source_frame_at(clip, Frame(end), sequence.timebase);
    let src_start = src_start_frame
        .to_seconds(clip.media_timebase)
        .max(0.0);
    let src_span = (src_end_frame.0 - src_start_frame.0).max(1) as f64
        * clip.media_timebase.frame_duration_secs();
    let rel = eval_frame - clip.timeline_in.0;
    let grade = grade_filter(color_grade(&clip.effects), rel);
    let xform = transform(&clip.effects).cloned().unwrap_or_else(Transform::identity);
    let opacity = (xform.opacity.value_at(rel) * opacity_scale).clamp(0.0, 1.0);
    let scale_x = xform.scale_x.value_at(rel).abs().max(0.01);
    let scale_y = xform.scale_y.value_at(rel).abs().max(0.01);
    let pos_x = xform.position_x.value_at(rel);
    let pos_y = xform.position_y.value_at(rel);
    let rotation = xform.rotation_deg.value_at(rel);
    let full = (scale_x - 1.0).abs() < 0.015
        && (scale_y - 1.0).abs() < 0.015
        && pos_x.abs() < 0.5
        && pos_y.abs() < 0.5
        && rotation.abs() < 0.05
        && opacity > 0.999;

    let index = graph.add_input(&resolved.to_string_lossy());
    let fitted = graph.lab();
    let speed = if src_span > 0.001 { dur / src_span } else { 1.0 };
    graph.filters.push(format!(
        "[{index}:v]trim=start={}:end={},setpts={speed}*(PTS-STARTPTS),fps={fps},{grade}scale={width}:{height}:force_original_aspect_ratio=decrease,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1,trim=duration={},setpts=PTS-STARTPTS[{fitted}]",
        secs(src_start),
        secs(src_start + src_span.max(1.0 / 1000.0)),
        secs(dur),
        speed = format!("{speed:.6}"),
        grade = grade,
    ));
    if full {
        let out = graph.lab();
        graph.filters.push(format!(
            "[{fitted}]format=yuv420p,fps={fps},trim=duration={},setpts=PTS-STARTPTS[{out}]",
            secs(dur)
        ));
        return Ok(out);
    }
    let sw = even_dim((width as f32 * scale_x).round().max(2.0) as u32);
    let sh = even_dim((height as f32 * scale_y).round().max(2.0) as u32);
    let scaled = graph.lab();
    let mut chain = format!("[{fitted}]scale={sw}:{sh}:flags=bilinear,setsar=1");
    if rotation.abs() > 0.05 {
        let radians = rotation.to_radians();
        chain.push_str(&format!(
            ",rotate={radians:.6}:ow=rotw({radians:.6}):oh=roth({radians:.6}):c=none:fillcolor=black@0"
        ));
    }
    chain.push_str(&format!(
        ",format=rgba,colorchannelmixer=aa={opacity:.4}[{scaled}]"
    ));
    graph.filters.push(chain);
    let plate = graph.lab();
    graph.filters.push(format!(
        "color=c=black@0:s={width}x{height}:r={fps}:d={},format=rgba[{plate}]",
        secs(dur)
    ));
    let out = graph.lab();
    graph.filters.push(format!(
        "[{plate}][{scaled}]overlay=x='(main_w-overlay_w)/2+({pos_x:.2})':y='(main_h-overlay_h)/2-({pos_y:.2})':format=auto:eof_action=pass:shortest=1,format=yuva420p,setsar=1[{out}]"
    ));
    Ok(out)
}

fn grade_filter(grade: Option<&ColorGrade>, rel: i64) -> String {
    let Some(grade) = grade else {
        return String::new();
    };
    let exposure = grade.exposure.value_at(rel);
    let contrast = grade.contrast.value_at(rel).clamp(0.0, 4.0);
    let saturation = grade.saturation.value_at(rel).clamp(0.0, 4.0);
    let shadows = (grade.shadows.value_at(rel) * 0.35).clamp(-1.0, 1.0);
    let highlights = (grade.highlights.value_at(rel) * 0.35).clamp(-1.0, 1.0);
    let temperature = grade.temperature.value_at(rel).clamp(-1.0, 1.0);
    let tint = grade.tint.value_at(rel).clamp(-1.0, 1.0);
    let neutral = exposure.abs() < 1.0e-3
        && (contrast - 1.0).abs() < 1.0e-3
        && (saturation - 1.0).abs() < 1.0e-3
        && shadows.abs() < 1.0e-3
        && highlights.abs() < 1.0e-3
        && temperature.abs() < 1.0e-3
        && tint.abs() < 1.0e-3;
    if neutral {
        return String::new();
    }
    let rm = (temperature * 0.25 + tint * 0.08).clamp(-1.0, 1.0);
    let gm = (-tint * 0.20).clamp(-1.0, 1.0);
    let bm = (-temperature * 0.25 + tint * 0.08).clamp(-1.0, 1.0);
    format!(
        "exposure=exposure={exposure:.4}:black=0,eq=contrast={contrast:.4}:saturation={saturation:.4},colorbalance=rs={shadows:.4}:gs={shadows:.4}:bs={shadows:.4}:rh={highlights:.4}:gh={highlights:.4}:bh={highlights:.4}:rm={rm:.4}:gm={gm:.4}:bm={bm:.4},"
    )
}

fn slice_audio(
    graph: &mut Graph,
    pieces: &[WavPiece],
    start: i64,
    end: i64,
    timebase: Timebase,
    dur: f64,
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
        graph.filters.push(format!(
            "[{index}:a]atrim=start={}:end={},asetpts=PTS-STARTPTS,volume={:.4},adelay={}|{},aformat=sample_rates=48000:channel_layouts=stereo[{label}]",
            secs(src),
            secs(src + src_dur.max(0.01)),
            piece.gain.clamp(0.0, 4.0),
            delay_ms.round() as i64,
            delay_ms.round() as i64,
        ));
        layers.push(label);
    }
    let mixed = if layers.is_empty() {
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
    let solos = sequence
        .tracks
        .iter()
        .any(|track| track.kind == TrackKind::Audio && track.solo);
    let mut pieces = Vec::new();
    for track in &sequence.tracks {
        if track.kind != TrackKind::Audio || track.muted {
            continue;
        }
        if solos && !track.solo {
            continue;
        }
        for clip in &track.clips {
            if !clip.enabled || clip.timeline_out.0 <= from || clip.timeline_in.0 >= to {
                continue;
            }
            let Some(asset) = clip
                .media_id
                .and_then(|id| media.iter().find(|item| item.id == id))
            else {
                continue;
            };
            if !asset.has_audio {
                continue;
            }
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
                timeline_in: clip.timeline_in.0,
                timeline_out: clip.timeline_out.0,
                source_at_in: at_in.to_seconds(clip.media_timebase).max(0.0),
                seconds_per_frame,
                gain: clip.volume.clamp(0.0, 4.0),
            });
        }
    }
    pieces
}

fn burn_drawtext(graph: &mut Graph, input: &str, cues: &[(f64, f64, String)], font: &Path) -> String {
    let mut label = input.to_string();
    let font = escape_filter_path(&font.to_string_lossy());
    for (start, end, text) in cues {
        if *end <= *start {
            continue;
        }
        let text = escape_drawtext(text);
        if text.is_empty() {
            continue;
        }
        let next = graph.lab();
        graph.filters.push(format!(
            "[{label}]drawtext=fontfile='{font}':text='{text}':fontsize=28:fontcolor=white:borderw=2:bordercolor=black:x=(w-text_w)/2:y=h-th-48:enable='between(t\\,{start:.3}\\,{end:.3})'[{next}]"
        ));
        label = next;
    }
    label
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
            let start = frames_secs(cue.timeline_in.0.max(range_in) - range_in, sequence.timebase);
            let end = frames_secs(cue.timeline_out.0.min(range_out) - range_in, sequence.timebase);
            if end > start && !cue.text.trim().is_empty() {
                cues.push((start, end, cue.text.clone()));
            }
        }
    }
    cues.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    cues
}

fn video_boundaries(sequence: &Sequence, range_in: i64, range_out: i64) -> Vec<i64> {
    let mut marks = vec![range_in, range_out];
    let transitions = transition_ranges(sequence);
    for track in sequence
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
    {
        for clip in &track.clips {
            push_mark(&mut marks, clip.timeline_in.0, range_in, range_out, &transitions);
            push_mark(&mut marks, clip.timeline_out.0, range_in, range_out, &transitions);
            for effect in &clip.effects {
                for frame in effect_key_frames(effect, clip.timeline_in.0) {
                    push_mark(&mut marks, frame, range_in, range_out, &transitions);
                }
            }
        }
        for transition in &track.transitions {
            if let Some(left) = track.clips.iter().find(|clip| clip.id == transition.left_clip) {
                let (start, end) = transition.range(left.timeline_out);
                push_edge(&mut marks, start.0, range_in, range_out);
                push_edge(&mut marks, end.0, range_in, range_out);
            }
        }
    }
    marks.sort_unstable();
    marks.dedup();
    marks
}

fn push_mark(marks: &mut Vec<i64>, frame: i64, inn: i64, out: i64, transitions: &[(i64, i64)]) {
    if frame <= inn || frame >= out {
        return;
    }
    if transitions
        .iter()
        .any(|(start, end)| frame > *start && frame < *end)
    {
        return;
    }
    marks.push(frame);
}

fn push_edge(marks: &mut Vec<i64>, frame: i64, inn: i64, out: i64) {
    if frame > inn && frame < out {
        marks.push(frame);
    }
}

fn transition_ranges(sequence: &Sequence) -> Vec<(i64, i64)> {
    let mut ranges = Vec::new();
    for track in &sequence.tracks {
        for transition in &track.transitions {
            if let Some(left) = track.clips.iter().find(|clip| clip.id == transition.left_clip) {
                let (start, end) = transition.range(left.timeline_out);
                if end.0 > start.0 {
                    ranges.push((start.0, end.0));
                }
            }
        }
    }
    ranges
}

fn effect_key_frames(effect: &Effect, timeline_in: i64) -> Vec<i64> {
    let mut frames = Vec::new();
    match effect {
        Effect::Color(grade) => {
            for anim in [
                &grade.exposure,
                &grade.contrast,
                &grade.highlights,
                &grade.shadows,
                &grade.temperature,
                &grade.tint,
                &grade.saturation,
            ] {
                push_keys(&mut frames, anim, timeline_in);
            }
        }
        Effect::Transform(xform) => {
            for anim in [
                &xform.position_x,
                &xform.position_y,
                &xform.scale_x,
                &xform.scale_y,
                &xform.rotation_deg,
                &xform.anchor_x,
                &xform.anchor_y,
                &xform.opacity,
            ] {
                push_keys(&mut frames, anim, timeline_in);
            }
        }
    }
    frames
}

fn push_keys(frames: &mut Vec<i64>, anim: &AnimatedF32, timeline_in: i64) {
    for key in &anim.keys {
        frames.push(timeline_in + key.frame);
    }
}

fn pairs(marks: &[i64]) -> Vec<(i64, i64)> {
    marks.windows(2).map(|pair| (pair[0], pair[1])).collect()
}

fn subdivide_animated(sequence: &Sequence, slices: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    const STEP: i64 = 6;
    let mut out = Vec::new();
    for (start, end) in slices {
        if end <= start {
            continue;
        }
        let inside_transition = sequence.tracks.iter().any(|track| {
            transition_on(track, start, end).is_some_and(|blend| blend.exact)
        });
        if inside_transition || !slice_changes(sequence, start, end) || end - start <= STEP {
            out.push((start, end));
            continue;
        }
        let mut cursor = start;
        while cursor < end {
            let next = (cursor + STEP).min(end);
            out.push((cursor, next));
            cursor = next;
        }
    }
    out
}

fn slice_changes(sequence: &Sequence, start: i64, end: i64) -> bool {
    for track in sequence
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video && track_visible(track, &sequence.tracks))
    {
        for clip in track.clips.iter().filter(|clip| {
            clip.enabled && clip.timeline_in.0 < end && clip.timeline_out.0 > start
        }) {
            let rel0 = start - clip.timeline_in.0;
            let rel1 = end - clip.timeline_in.0;
            for effect in &clip.effects {
                if effect_changes(effect, rel0, rel1) {
                    return true;
                }
            }
        }
    }
    false
}

fn effect_changes(effect: &Effect, rel0: i64, rel1: i64) -> bool {
    match effect {
        Effect::Color(grade) => [
            &grade.exposure,
            &grade.contrast,
            &grade.highlights,
            &grade.shadows,
            &grade.temperature,
            &grade.tint,
            &grade.saturation,
        ]
        .into_iter()
        .any(|anim| anim_changes(anim, rel0, rel1)),
        Effect::Transform(xform) => [
            &xform.position_x,
            &xform.position_y,
            &xform.scale_x,
            &xform.scale_y,
            &xform.rotation_deg,
            &xform.opacity,
        ]
        .into_iter()
        .any(|anim| anim_changes(anim, rel0, rel1)),
    }
}

fn anim_changes(anim: &AnimatedF32, rel0: i64, rel1: i64) -> bool {
    if anim.keys.is_empty() {
        return false;
    }
    (anim.value_at(rel0) - anim.value_at(rel1)).abs() > 1.0e-3
        || anim.keys.iter().any(|key| key.frame > rel0 && key.frame < rel1)
}

fn track_visible(track: &Track, tracks: &[Track]) -> bool {
    if track.muted {
        return false;
    }
    let any_solo = tracks.iter().any(|item| item.kind == track.kind && item.solo);
    if any_solo {
        track.solo
    } else {
        true
    }
}

fn export_bounds(sequence: &Sequence, range: ExportRange) -> Result<(i64, i64), String> {
    let end = sequence.end_frame().0.max(0);
    let (inn, out) = match range {
        ExportRange::InOut => {
            let inn = sequence.in_point.unwrap_or(Frame::ZERO).0.max(0);
            let out = sequence.out_point.map(|frame| frame.0).unwrap_or(end).max(inn);
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

fn escape_drawtext(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '\\' | ':' | '\'' | '%' | ',' | '[' | ']' | ';' => {
                out.push('\\');
                out.push(ch);
            }
            '\n' | '\r' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

fn escape_filter_path(path: &str) -> String {
    path.replace('\\', "\\\\")
        .replace(':', "\\:")
        .replace('\'', "\\'")
}

fn codec_args(codec: &str) -> Vec<String> {
    match codec.trim().to_ascii_lowercase().as_str() {
        "h.265" | "h265" | "hevc" => split_args(
            "-c:v libx265 -tag:v hvc1 -preset veryfast -crf 20 -pix_fmt yuv420p -c:a aac -b:a 192k",
        ),
        "prores 422" => split_args("-c:v prores_ks -profile:v 2 -pix_fmt yuv422p10le -c:a aac -b:a 192k"),
        "prores 4444" => {
            split_args("-c:v prores_ks -profile:v 4 -pix_fmt yuva444p10le -c:a aac -b:a 192k")
        }
        "dnxhr hq" | "dnxhr" => {
            split_args("-c:v dnxhd -profile:v dnxhr_hq -pix_fmt yuv422p -c:a aac -b:a 192k")
        }
        _ => split_args(
            "-c:v libx264 -preset veryfast -crf 18 -pix_fmt yuv420p -c:a aac -b:a 192k -movflags +faststart",
        ),
    }
}

fn split_args(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

fn container_name(container: &str) -> &'static str {
    match container.trim().to_ascii_lowercase().as_str() {
        "mov" => "mov",
        "mxf" => "mxf",
        _ => "mp4",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::{
        Clip, ClipId, CueId, MediaId, SequenceId, TrackId, Transition, TransitionAlign,
        TransitionId,
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
        audio.volume = 0.25;
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
        let script = plan_encode(
            &sequence,
            &media,
            ExportRange::InOut,
            "H.264",
            "mp4",
            true,
        )
        .unwrap();
        assert!(script.filter.contains("exposure=exposure=0.5000"));
        assert!(script.filter.contains("xfade=transition=fade"));
        assert!(script.filter.contains("overlay="));
        assert!(script.filter.contains("volume=0.2500"));
        assert!(script.filter.contains("drawtext="));
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
        )
        .unwrap();
        assert!(script.filter.contains("color=c=black"));
        assert!(!script.inputs.iter().any(|path| path.ends_with("picture.mp4")));
        assert!(script.inputs.iter().any(|path| path.ends_with("keep.wav")));
        assert!(!script.inputs.iter().any(|path| path.ends_with("drop.wav")));
        assert!(script.srt.is_empty());
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
        audio.volume = 0.5;
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
        let dir = std::env::temp_dir().join(format!("meridian-whisper-export-{}", std::process::id()));
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
                "-y", "-hide_banner", "-loglevel", "error",
                "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=24:duration=6",
                "-i",
            ])
            .arg(&wav)
            .args([
                "-shortest", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac",
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
        audio.volume = 0.8;
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
        assert!(joined.contains("ridge") || joined.contains("came"), "{joined}");
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
        )
        .unwrap();
        assert!(script.filter.contains("exposure=exposure=0.4500"));
        assert!(script.filter.contains("drawtext="));
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
                "-y", "-hide_banner", "-loglevel", "error",
                "-ss", "1.0", "-i",
            ])
            .arg(&output)
            .args(["-frames:v", "1"])
            .arg(&frame)
            .status()
            .unwrap();
        assert!(dumped.success());
        let raw = dir.join("frame.rgb");
        let dumped = std::process::Command::new("ffmpeg")
            .args([
                "-y", "-hide_banner", "-loglevel", "error",
                "-i",
            ])
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
        // drawtext sits at y = h - text_h - 48, about rows 96–132 on a 180p frame.
        let (picture, _) = stats(20, 70);
        let (burned, burned_dev) = stats(96, 136);
        if std::env::var_os("MERIDIAN_KEEP_EXPORT").is_some() {
            let _ = std::fs::copy(&output, "/tmp/meridian-whisper-export.mp4");
            let _ = std::fs::copy(&frame, "/tmp/meridian-whisper-frame.png");
        }
        assert!(picture > 40.0, "graded picture is unexpectedly dark: {picture}");
        assert!(
            burned_dev > 35.0,
            "caption band is flat (mean {burned}, stddev {burned_dev}); cues: {joined}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
