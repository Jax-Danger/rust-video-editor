//! Timeline audio output.
//!
//! With `--features ffmpeg`, audible clips are mixed to stereo PCM by the
//! ffmpeg CLI and played through rodio. One worker decodes about two seconds
//! at a time, so playback does not spawn a process per frame. The default
//! build keeps the same meters and reports that output is compiled into the
//! ffmpeg feature.

use editor_core::{source_frame_at, Frame, MediaAsset, Sequence, TrackKind};
use editor_media::resolve_media_path;

#[cfg(feature = "ffmpeg")]
use editor_media::{decode_audio, AudioRequest, AUDIO_CHANNELS, AUDIO_RATE};

const CHUNK_SECS: f64 = 2.0;

/// One audible region, in sequence frames, with the source mapping at its in point.
#[derive(Clone, Debug)]
pub struct AudioPiece {
    pub path: String,
    pub timeline_in: i64,
    pub timeline_out: i64,
    pub source_at_in: f64,
    pub seconds_per_frame: f64,
    pub gain: f32,
}

pub struct AudioEngine {
    peaks: [f32; 2],
    status: String,
    badge: &'static str,
    #[cfg(feature = "ffmpeg")]
    live: Option<Live>,
}

#[cfg(feature = "ffmpeg")]
struct Live {
    sink: rodio::MixerDeviceSink,
    player: rodio::Player,
    tx: std::sync::mpsc::Sender<MixJob>,
    rx: std::sync::mpsc::Receiver<MixDone>,
    worker: Option<std::thread::JoinHandle<()>>,
    next_id: u64,
    generation: u64,
    inflight: Option<u64>,
    queued: Option<MixJob>,
    clock: Option<Clock>,
    scheduled_until: i64,
    sequence_end: i64,
    fps: f64,
    pieces: Vec<AudioPiece>,
    armed: bool,
}

#[cfg(feature = "ffmpeg")]
struct Clock {
    origin_frame: i64,
    started: std::time::Instant,
    fps: f64,
}

#[cfg(feature = "ffmpeg")]
struct MixJob {
    id: u64,
    generation: u64,
    start_frame: i64,
    frames: i64,
    pieces: Vec<AudioPiece>,
    fps: f64,
}

#[cfg(feature = "ffmpeg")]
struct MixDone {
    id: u64,
    generation: u64,
    start_frame: i64,
    samples: Result<Vec<f32>, String>,
    peaks: [f32; 2],
}

impl AudioEngine {
    pub fn new() -> Self {
        #[cfg(feature = "ffmpeg")]
        {
            match open_output() {
                Ok(live) => Self {
                    peaks: [0.0, 0.0],
                    status: "Audio ready.".into(),
                    badge: "Audio",
                    live: Some(live),
                },
                Err(err) => Self {
                    peaks: [0.0, 0.0],
                    status: format!("Audio device unavailable: {err}"),
                    badge: "No device",
                    live: None,
                },
            }
        }
        #[cfg(not(feature = "ffmpeg"))]
        {
            Self {
                peaks: [0.0, 0.0],
                status: "Audio output is compiled into --features ffmpeg (rodio plays ffmpeg PCM)."
                    .into(),
                badge: "No audio",
            }
        }
    }

    pub fn peaks(&self) -> [f32; 2] {
        self.peaks
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn badge(&self) -> &'static str {
        self.badge
    }

    /// Picture should wait on the audio clock instead of free-running.
    pub fn drives_picture(&self) -> bool {
        #[cfg(feature = "ffmpeg")]
        {
            self.live.as_ref().is_some_and(|live| live.armed)
        }
        #[cfg(not(feature = "ffmpeg"))]
        {
            false
        }
    }

    pub fn stop(&mut self) {
        self.peaks = [0.0, 0.0];
        #[cfg(feature = "ffmpeg")]
        if let Some(live) = self.live.as_mut() {
            live.generation = live.generation.saturating_add(1);
            live.armed = false;
            live.clock = None;
            live.queued = None;
            live.player = rodio::Player::connect_new(live.sink.mixer());
            self.badge = "Audio";
            self.status = "Audio ready.".into();
        }
    }

    pub fn begin(&mut self, frame: i64, end: i64, fps: f64, pieces: Vec<AudioPiece>) {
        self.stop();
        #[cfg(not(feature = "ffmpeg"))]
        {
            let _ = (frame, end, fps, pieces);
        }
        #[cfg(feature = "ffmpeg")]
        {
            let Some(live) = self.live.as_mut() else {
                return;
            };
            live.pieces = pieces;
            live.fps = fps.max(1.0);
            live.sequence_end = end.max(0);
            live.scheduled_until = frame.max(0);
            live.clock = None;
            live.armed = live.scheduled_until < live.sequence_end
                && live.pieces.iter().any(|piece| {
                    piece.timeline_out > live.scheduled_until && piece.timeline_in < live.sequence_end
                });
            if live.armed {
                self.badge = "Buffering";
                self.status = "Audio buffering…".into();
                ensure_queued(live);
            } else if live.pieces.is_empty() {
                self.badge = "Silent";
                self.status = "No audible clips. Picture still plays.".into();
            }
        }
    }

    /// Poll the decoder and, once samples are queued, return the audio-clock frame.
    pub fn pump(&mut self) -> Option<i64> {
        self.peaks[0] *= 0.9;
        self.peaks[1] *= 0.9;
        #[cfg(feature = "ffmpeg")]
        {
            let status = self.live.as_mut().and_then(|live| poll_live(live));
            if let Some((badge, message, peaks)) = status {
                self.badge = badge;
                if let Some(message) = message {
                    self.status = message;
                }
                if let Some(peaks) = peaks {
                    self.peaks[0] = self.peaks[0].max(peaks[0]);
                    self.peaks[1] = self.peaks[1].max(peaks[1]);
                }
            }
            return self.live.as_ref().and_then(|live| {
                live.clock.as_ref().map(|clock| {
                    clock.origin_frame
                        + (clock.started.elapsed().as_secs_f64() * clock.fps).floor() as i64
                })
            });
        }
        #[cfg(not(feature = "ffmpeg"))]
        {
            None
        }
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        #[cfg(feature = "ffmpeg")]
        if let Some(mut live) = self.live.take() {
            let _ = live.tx;
            if let Some(worker) = live.worker.take() {
                let _ = worker.join();
            }
        }
    }
}

pub fn collect_pieces(sequence: &Sequence, media: &[MediaAsset], from: i64, to: i64) -> Vec<AudioPiece> {
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
            let Some(media_id) = clip.media_id else {
                continue;
            };
            let Some(asset) = media.iter().find(|item| item.id == media_id) else {
                continue;
            };
            if !asset.has_audio {
                continue;
            }
            let resolved = resolve_media_path(&asset.path);
            if !resolved.is_file() {
                continue;
            }
            let at_in = source_frame_at(clip, clip.timeline_in, sequence.timebase);
            let at_next = source_frame_at(clip, Frame(clip.timeline_in.0 + 1), sequence.timebase);
            let mut seconds_per_frame =
                (at_next.0 - at_in.0) as f64 * clip.media_timebase.frame_duration_secs();
            if seconds_per_frame <= 0.0 {
                seconds_per_frame = sequence.timebase.frame_duration_secs();
            }
            pieces.push(AudioPiece {
                path: resolved.to_string_lossy().into_owned(),
                timeline_in: clip.timeline_in.0,
                timeline_out: clip.timeline_out.0,
                source_at_in: at_in.to_seconds(clip.media_timebase),
                seconds_per_frame,
                gain: clip.volume.clamp(0.0, 4.0),
            });
        }
    }
    pieces
}

#[cfg(feature = "ffmpeg")]
fn open_output() -> Result<Live, String> {
    let sink = rodio::DeviceSinkBuilder::open_default_sink().map_err(|err| err.to_string())?;
    let player = rodio::Player::connect_new(sink.mixer());
    let (tx, job_rx) = std::sync::mpsc::channel::<MixJob>();
    let (done_tx, rx) = std::sync::mpsc::channel::<MixDone>();
    let worker = std::thread::spawn(move || worker_loop(job_rx, done_tx));
    Ok(Live {
        sink,
        player,
        tx,
        rx,
        worker: Some(worker),
        next_id: 1,
        generation: 1,
        inflight: None,
        queued: None,
        clock: None,
        scheduled_until: 0,
        sequence_end: 0,
        fps: 24.0,
        pieces: Vec::new(),
        armed: false,
    })
}

#[cfg(feature = "ffmpeg")]
fn poll_live(live: &mut Live) -> Option<(&'static str, Option<String>, Option<[f32; 2]>)> {
    let mut note = None;
    loop {
        let done = match live.rx.try_recv() {
            Ok(done) => done,
            Err(std::sync::mpsc::TryRecvError::Empty) => break,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                live.armed = false;
                live.inflight = None;
                return Some(("No audio", Some("Audio worker stopped.".into()), None));
            }
        };
        if live.inflight == Some(done.id) {
            live.inflight = None;
        }
        if done.generation != live.generation {
            continue;
        }
        match done.samples {
            Ok(samples) if !samples.is_empty() => {
                let peaks = done.peaks;
                let buffer = rodio::buffer::SamplesBuffer::new(
                    std::num::NonZeroU16::new(AUDIO_CHANNELS).expect("channels"),
                    std::num::NonZeroU32::new(AUDIO_RATE).expect("rate"),
                    samples,
                );
                live.player.append(buffer);
                if live.clock.is_none() {
                    live.clock = Some(Clock {
                        origin_frame: done.start_frame,
                        started: std::time::Instant::now(),
                        fps: live.fps,
                    });
                }
                note = Some(("Audio", Some("Audio playing.".into()), Some(peaks)));
            }
            Ok(_) => {}
            Err(err) => {
                live.armed = false;
                live.queued = None;
                return Some(("Audio", Some(format!("Audio: {err}")), None));
            }
        }
    }
    ensure_queued(live);
    note
}

#[cfg(feature = "ffmpeg")]
fn ensure_queued(live: &mut Live) {
    if !live.armed {
        return;
    }
    if live.inflight.is_none() {
        let job = if let Some(job) = live.queued.take() {
            Some(job)
        } else {
            take_job(live)
        };
        if let Some(job) = job {
            let id = job.id;
            if live.tx.send(job).is_ok() {
                live.inflight = Some(id);
            }
        } else if live.clock.is_some() && live.player.empty() {
            live.armed = false;
        }
    }
    if live.queued.is_none() && live.scheduled_until < live.sequence_end {
        live.queued = take_job(live);
    }
}

#[cfg(feature = "ffmpeg")]
fn take_job(live: &mut Live) -> Option<MixJob> {
    if live.scheduled_until >= live.sequence_end {
        return None;
    }
    let frames = chunk_frames(live.fps)
        .min(live.sequence_end - live.scheduled_until)
        .max(1);
    let start = live.scheduled_until;
    live.scheduled_until += frames;
    let id = live.next_id;
    live.next_id = live.next_id.saturating_add(1);
    Some(MixJob {
        id,
        generation: live.generation,
        start_frame: start,
        frames,
        pieces: live.pieces.clone(),
        fps: live.fps,
    })
}

#[cfg(feature = "ffmpeg")]
fn chunk_frames(fps: f64) -> i64 {
    (CHUNK_SECS * fps.max(1.0)).round().clamp(1.0, 240.0) as i64
}

#[cfg(feature = "ffmpeg")]
fn worker_loop(
    rx: std::sync::mpsc::Receiver<MixJob>,
    tx: std::sync::mpsc::Sender<MixDone>,
) {
    while let Ok(job) = rx.recv() {
        let mixed = mix_chunk(&job);
        let peaks = mixed.as_ref().map(|samples| stereo_peak(samples)).unwrap_or([0.0, 0.0]);
        let done = MixDone {
            id: job.id,
            generation: job.generation,
            start_frame: job.start_frame,
            samples: mixed,
            peaks,
        };
        if tx.send(done).is_err() {
            break;
        }
    }
}

#[cfg(feature = "ffmpeg")]
fn mix_chunk(job: &MixJob) -> Result<Vec<f32>, String> {
    let fps = job.fps.max(1.0);
    let duration = job.frames as f64 / fps;
    let sample_count = (duration * f64::from(AUDIO_RATE)).round().max(1.0) as usize;
    let mut mix = vec![0.0f32; sample_count * AUDIO_CHANNELS as usize];
    let mut any = false;
    let mut last_error = None;
    let end = job.start_frame + job.frames;
    for piece in &job.pieces {
        let overlap_in = job.start_frame.max(piece.timeline_in);
        let overlap_out = end.min(piece.timeline_out);
        if overlap_out <= overlap_in {
            continue;
        }
        let file_start =
            piece.source_at_in + (overlap_in - piece.timeline_in) as f64 * piece.seconds_per_frame;
        let file_duration = (overlap_out - overlap_in) as f64 * piece.seconds_per_frame;
        if file_duration <= 0.001 {
            continue;
        }
        let request = match AudioRequest::new(&piece.path, file_start.max(0.0), file_duration.min(8.0))
        {
            Ok(request) => request,
            Err(err) => {
                last_error = Some(err.to_string());
                continue;
            }
        };
        let decoded = match decode_audio(&request) {
            Ok(samples) => samples,
            Err(err) => {
                last_error = Some(err.to_string());
                continue;
            }
        };
        any = true;
        let offset = ((overlap_in - job.start_frame) as f64 / fps * f64::from(AUDIO_RATE)).round()
            as usize
            * AUDIO_CHANNELS as usize;
        let gain = piece.gain;
        for (index, sample) in decoded.iter().enumerate() {
            let at = offset + index;
            if at >= mix.len() {
                break;
            }
            mix[at] = (mix[at] + sample * gain).clamp(-1.0, 1.0);
        }
    }
    if !any && last_error.is_some() && job.pieces.iter().any(|piece| {
        piece.timeline_out > job.start_frame && piece.timeline_in < end
    }) {
        return Err(last_error.unwrap_or_else(|| "audio decode failed".into()));
    }
    Ok(mix)
}

#[cfg(feature = "ffmpeg")]
fn stereo_peak(samples: &[f32]) -> [f32; 2] {
    let mut peak = [0.0f32; 2];
    for (index, sample) in samples.iter().enumerate() {
        let channel = index % 2;
        peak[channel] = peak[channel].max(sample.abs());
    }
    peak
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::{Clip, MediaId, SequenceId, Timebase, TrackId};

    #[test]
    fn collect_skips_muted_and_offline_audio() {
        let mut sequence = Sequence::new(SequenceId(1), "T", 1920, 1080, Timebase::fps_24());
        sequence.add_track(TrackId(2), TrackKind::Audio, "A1");
        let mut clip = Clip::basic(3, 0, 48);
        clip.media_id = Some(MediaId(9));
        sequence.tracks[0].clips = vec![clip];
        sequence.tracks[0].muted = true;
        let media = vec![MediaAsset {
            id: MediaId(9),
            bin_id: editor_core::BinId(1),
            name: "tone.wav".into(),
            path: "missing-tone.wav".into(),
            duration: Frame(48),
            timebase: Timebase::fps_24(),
            width: None,
            height: None,
            video_codec: None,
            audio_codec: Some("pcm".into()),
            audio_channels: Some(2),
            sample_rate: Some(48_000),
            has_video: false,
            has_audio: true,
            offline: true,
        }];
        assert!(collect_pieces(&sequence, &media, 0, 48).is_empty());
        sequence.tracks[0].muted = false;
        assert!(collect_pieces(&sequence, &media, 0, 48).is_empty());
    }

    #[test]
    fn collect_respects_clip_gain_and_solo() {
        let dir = std::env::temp_dir().join(format!("meridian-gain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let keep = dir.join("keep.wav");
        let other = dir.join("other.wav");
        std::fs::write(&keep, b"wav").unwrap();
        std::fs::write(&other, b"wav").unwrap();
        let mut sequence = Sequence::new(SequenceId(1), "T", 1920, 1080, Timebase::fps_24());
        sequence.add_track(TrackId(2), TrackKind::Audio, "A1");
        sequence.add_track(TrackId(3), TrackKind::Audio, "A2");
        let mut quiet = Clip::basic(4, 0, 24);
        quiet.media_id = Some(MediaId(1));
        quiet.volume = 0.35;
        let mut loud = Clip::basic(5, 0, 24);
        loud.media_id = Some(MediaId(2));
        loud.volume = 1.5;
        sequence.tracks[0].clips = vec![quiet];
        sequence.tracks[1].clips = vec![loud];
        let media = vec![
            tone(MediaId(1), &keep),
            tone(MediaId(2), &other),
        ];
        let mixed = collect_pieces(&sequence, &media, 0, 24);
        assert_eq!(mixed.len(), 2);
        assert!((mixed[0].gain - 0.35).abs() < 1.0e-5);
        sequence.tracks[1].solo = true;
        let solo = collect_pieces(&sequence, &media, 0, 24);
        assert_eq!(solo.len(), 1);
        assert!((solo[0].gain - 1.5).abs() < 1.0e-5);
        assert!(solo[0].path.ends_with("other.wav"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tone(id: MediaId, path: &std::path::Path) -> MediaAsset {
        MediaAsset {
            id,
            bin_id: editor_core::BinId(1),
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            path: path.to_string_lossy().into_owned(),
            duration: Frame(48),
            timebase: Timebase::fps_24(),
            width: None,
            height: None,
            video_codec: None,
            audio_codec: Some("pcm".into()),
            audio_channels: Some(2),
            sample_rate: Some(48_000),
            has_video: false,
            has_audio: true,
            offline: false,
        }
    }
}
