//! Timeline audio output.
//!
//! With `--features ffmpeg`, clips are decoded to stereo PCM by the ffmpeg CLI
//! and played through rodio (ALSA, which PipeWire serves via `pipewire-alsa`).
//! One worker decodes about two seconds at a time. Fader, pan, mute, solo,
//! master, clip gain, EQ, and compressor — including keyframes — are applied
//! in the playback callback, so a mixer move is heard on the next few
//! milliseconds rather than the next decoded chunk. The default build keeps the
//! same meters and reports that output is compiled into the ffmpeg feature.

use editor_core::{
    audio_topology, mix_regions, multicam_audio_spans, source_frame_at, update_hold, BusState,
    Frame, MediaAsset, MulticamGroup, Sequence, TrackKind,
};
#[cfg(feature = "ffmpeg")]
use editor_core::{channel_clips, mix_frame, CompressorProcessor, EqProcessor};
use editor_media::resolve_media_path;

#[cfg(feature = "ffmpeg")]
use editor_media::{decode_audio, AudioRequest, AUDIO_CHANNELS, AUDIO_RATE};

#[cfg(feature = "ffmpeg")]
const CHUNK_SECS: f64 = 2.0;

/// One audible region, in sequence frames, with the source mapping at its in point.
#[derive(Clone, Debug)]
#[cfg_attr(not(feature = "ffmpeg"), allow(dead_code))]
pub struct AudioPiece {
    pub path: String,
    pub timeline_in: i64,
    pub timeline_out: i64,
    pub source_at_in: f64,
    pub seconds_per_frame: f64,
    /// Clip gain × track fader at the clip in-point. Playback applies the live
    /// bus; Whisper and the mix tests read these baked values.
    #[cfg_attr(not(feature = "whisper"), allow(dead_code))]
    pub gain: f32,
    #[cfg_attr(not(feature = "whisper"), allow(dead_code))]
    pub pan: f32,
    #[cfg_attr(not(feature = "whisper"), allow(dead_code))]
    pub gain_keys: Vec<editor_core::GainKey>,
    pub track_id: u64,
    pub clip_id: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct MeterReadout {
    pub peak: [f32; 2],
    pub rms: [f32; 2],
    pub hold: [f32; 2],
    /// Latched when a channel reaches full scale. Cleared from the strip.
    pub clip: bool,
}

impl Default for MeterReadout {
    fn default() -> Self {
        Self {
            peak: [0.0, 0.0],
            rms: [0.0, 0.0],
            hold: [0.0, 0.0],
            clip: false,
        }
    }
}

pub struct AudioEngine {
    peaks: [f32; 2],
    status: String,
    badge: &'static str,
    topology: u64,
    master_meter: MeterReadout,
    track_meters: Vec<(u64, MeterReadout)>,
    last_meter: Option<std::time::Instant>,
    seen_generation: u64,
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
    control: std::sync::Arc<MixControl>,
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
    frame_count: usize,
    pieces: Result<Vec<DecodedPiece>, String>,
}

#[cfg(feature = "ffmpeg")]
struct DecodedPiece {
    clip_id: u64,
    track_id: u64,
    samples: Vec<f32>,
    origin_frame: f64,
    frame_offset: usize,
}

#[cfg(feature = "ffmpeg")]
struct MeterPub {
    generation: u64,
    master_peak: [f32; 2],
    master_rms: [f32; 2],
    master_clip: bool,
    tracks: Vec<(u64, [f32; 2], [f32; 2], bool)>,
}

#[cfg(feature = "ffmpeg")]
struct MixControl {
    params: std::sync::Mutex<std::sync::Arc<BusState>>,
    meters: std::sync::Mutex<MeterPub>,
}

#[cfg(feature = "ffmpeg")]
impl MixControl {
    fn new() -> Self {
        Self {
            params: std::sync::Mutex::new(std::sync::Arc::new(BusState {
                master: 1.0,
                tracks: Vec::new(),
                clips: Vec::new(),
            })),
            meters: std::sync::Mutex::new(MeterPub {
                generation: 0,
                master_peak: [0.0, 0.0],
                master_rms: [0.0, 0.0],
                master_clip: false,
                tracks: Vec::new(),
            }),
        }
    }

    fn set_bus(&self, bus: BusState) {
        if let Ok(mut guard) = self.params.lock() {
            *guard = std::sync::Arc::new(bus);
        }
    }
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
                    topology: 0,
                    master_meter: MeterReadout::default(),
                    track_meters: Vec::new(),
                    last_meter: None,
                    seen_generation: 0,
                    live: Some(live),
                },
                Err(err) => Self {
                    peaks: [0.0, 0.0],
                    status: format!("Audio device unavailable: {err}"),
                    badge: "No device",
                    topology: 0,
                    master_meter: MeterReadout::default(),
                    track_meters: Vec::new(),
                    last_meter: None,
                    seen_generation: 0,
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
                topology: 0,
                master_meter: MeterReadout::default(),
                track_meters: Vec::new(),
                last_meter: None,
                seen_generation: 0,
            }
        }
    }

    pub fn master_meter(&self) -> MeterReadout {
        self.master_meter
    }

    pub fn track_meter(&self, track_id: u64) -> MeterReadout {
        self.track_meters
            .iter()
            .find(|(id, _)| *id == track_id)
            .map(|(_, meter)| *meter)
            .unwrap_or_default()
    }

    pub fn clear_track_clip(&mut self, track_id: u64) {
        if let Some((_, meter)) = self.track_meters.iter_mut().find(|(id, _)| *id == track_id) {
            meter.clip = false;
        }
    }

    pub fn clear_master_clip(&mut self) {
        self.master_meter.clip = false;
    }

    pub fn meters_hot(&self) -> bool {
        let hot = |meter: MeterReadout| {
            meter.peak.iter().any(|v| *v > 0.01) || meter.hold.iter().any(|v| *v > 0.01)
        };
        hot(self.master_meter) || self.track_meters.iter().any(|(_, meter)| hot(*meter))
    }

    pub fn topology(&self) -> u64 {
        self.topology
    }

    pub fn set_topology(&mut self, topology: u64) {
        self.topology = topology;
    }

    pub fn set_bus(&mut self, bus: BusState) {
        #[cfg(feature = "ffmpeg")]
        if let Some(live) = self.live.as_ref() {
            live.control.set_bus(bus);
        }
        #[cfg(not(feature = "ffmpeg"))]
        {
            let _ = bus;
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
                    piece.timeline_out > live.scheduled_until
                        && piece.timeline_in < live.sequence_end
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
        self.ingest_meters();
        #[cfg(feature = "ffmpeg")]
        {
            let status = self.live.as_mut().and_then(|live| poll_live(live));
            if let Some((badge, message)) = status {
                self.badge = badge;
                if let Some(message) = message {
                    self.status = message;
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

    fn ingest_meters(&mut self) {
        let now = std::time::Instant::now();
        let dt = self
            .last_meter
            .map(|then| now.duration_since(then).as_secs_f32())
            .unwrap_or(0.0)
            .clamp(0.0, 0.1);
        self.last_meter = Some(now);
        let playing = self.drives_picture();
        #[cfg(feature = "ffmpeg")]
        let published = self.live.as_ref().and_then(|live| {
            live.control.meters.lock().ok().map(|meters| {
                (
                    meters.generation,
                    meters.master_peak,
                    meters.master_rms,
                    meters.master_clip,
                    meters.tracks.clone(),
                )
            })
        });
        #[cfg(not(feature = "ffmpeg"))]
        let published: Option<(
            u64,
            [f32; 2],
            [f32; 2],
            bool,
            Vec<(u64, [f32; 2], [f32; 2], bool)>,
        )> = None;
        if let Some((generation, peak, rms, master_clip, tracks)) = published {
            if generation != self.seen_generation && generation != 0 {
                self.seen_generation = generation;
                self.master_meter.peak = peak;
                self.master_meter.rms = rms;
                self.master_meter.clip |= master_clip;
                let previous = std::mem::take(&mut self.track_meters);
                for (id, peak, rms, clip) in tracks {
                    let old = previous
                        .iter()
                        .find(|(old, _)| *old == id)
                        .map(|(_, meter)| *meter);
                    let hold = old.map(|meter| meter.hold).unwrap_or([0.0, 0.0]);
                    let clip = old.map(|meter| meter.clip).unwrap_or(false) || clip;
                    self.track_meters.push((
                        id,
                        MeterReadout {
                            peak,
                            rms,
                            hold,
                            clip,
                        },
                    ));
                }
            } else if !playing {
                decay_readout(&mut self.master_meter, dt);
                for (_, meter) in &mut self.track_meters {
                    decay_readout(meter, dt);
                }
            }
        } else if !playing {
            decay_readout(&mut self.master_meter, dt);
            for (_, meter) in &mut self.track_meters {
                decay_readout(meter, dt);
            }
        }
        self.master_meter.hold[0] =
            update_hold(self.master_meter.hold[0], self.master_meter.peak[0], dt);
        self.master_meter.hold[1] =
            update_hold(self.master_meter.hold[1], self.master_meter.peak[1], dt);
        for (_, meter) in &mut self.track_meters {
            meter.hold[0] = update_hold(meter.hold[0], meter.peak[0], dt);
            meter.hold[1] = update_hold(meter.hold[1], meter.peak[1], dt);
        }
        self.peaks = self.master_meter.peak;
    }
}

fn decay_readout(meter: &mut MeterReadout, dt: f32) {
    let keep = (-3.5 * dt).exp();
    meter.peak[0] *= keep;
    meter.peak[1] *= keep;
    meter.rms[0] *= keep;
    meter.rms[1] *= keep;
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

pub fn collect_pieces(
    sequence: &Sequence,
    media: &[MediaAsset],
    groups: &[MulticamGroup],
    from: i64,
    to: i64,
) -> Vec<AudioPiece> {
    collect_filtered(sequence, media, groups, from, to, false)
}

/// Every audio clip with a file, including muted and non-solo tracks.
///
/// Playback decodes these and applies mute, solo, fader, and pan live.
pub fn collect_bus_pieces(
    sequence: &Sequence,
    media: &[MediaAsset],
    groups: &[MulticamGroup],
    from: i64,
    to: i64,
) -> Vec<AudioPiece> {
    collect_filtered(sequence, media, groups, from, to, true)
}

pub fn topology_of(sequence: &Sequence) -> u64 {
    audio_topology(sequence)
}

fn collect_filtered(
    sequence: &Sequence,
    media: &[MediaAsset],
    groups: &[MulticamGroup],
    from: i64,
    to: i64,
    include_silent: bool,
) -> Vec<AudioPiece> {
    let mut pieces = Vec::new();
    for region in mix_regions(sequence, from, to, include_silent) {
        let Some(clip) = sequence
            .tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Audio)
            .flat_map(|track| track.clips.iter())
            .find(|clip| clip.id.0 == region.clip_id)
        else {
            continue;
        };
        if let Some(spans) = multicam_audio_spans(clip, groups, media, sequence.timebase) {
            for span in spans {
                let Some(asset) = media.iter().find(|item| item.id == span.media_id) else {
                    continue;
                };
                if !asset.has_audio {
                    continue;
                }
                let resolved = resolve_media_path(&asset.path);
                if !resolved.is_file() {
                    continue;
                }
                pieces.push(AudioPiece {
                    path: resolved.to_string_lossy().into_owned(),
                    timeline_in: span.timeline_in,
                    timeline_out: span.timeline_out,
                    source_at_in: span.source_at_in,
                    seconds_per_frame: span.seconds_per_frame,
                    gain: region.gain,
                    pan: region.pan,
                    gain_keys: region.gain_keys.clone(),
                    track_id: region.track_id,
                    clip_id: region.clip_id,
                });
            }
            continue;
        }
        let Some(asset) = media.iter().find(|item| item.id == region.media_id) else {
            continue;
        };
        if !asset.has_audio {
            continue;
        }
        let resolved = resolve_media_path(&asset.path);
        if !resolved.is_file() {
            continue;
        }
        if clip.speed.mutes_audio() {
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
            timeline_in: region.timeline_in,
            timeline_out: region.timeline_out,
            source_at_in: at_in.to_seconds(clip.media_timebase),
            seconds_per_frame,
            gain: region.gain,
            pan: region.pan,
            gain_keys: region.gain_keys,
            track_id: region.track_id,
            clip_id: region.clip_id,
        });
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
        control: std::sync::Arc::new(MixControl::new()),
    })
}

#[cfg(feature = "ffmpeg")]
fn poll_live(live: &mut Live) -> Option<(&'static str, Option<String>)> {
    let mut note = None;
    loop {
        let done = match live.rx.try_recv() {
            Ok(done) => done,
            Err(std::sync::mpsc::TryRecvError::Empty) => break,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                live.armed = false;
                live.inflight = None;
                return Some(("No audio", Some("Audio worker stopped.".into())));
            }
        };
        if live.inflight == Some(done.id) {
            live.inflight = None;
        }
        if done.generation != live.generation {
            continue;
        }
        match done.pieces {
            Ok(pieces) => {
                let source = BusSource::new(
                    pieces,
                    done.frame_count,
                    live.fps,
                    std::sync::Arc::clone(&live.control),
                );
                live.player.append(source);
                if live.clock.is_none() {
                    live.clock = Some(Clock {
                        origin_frame: done.start_frame,
                        started: std::time::Instant::now(),
                        fps: live.fps,
                    });
                }
                note = Some(("Audio", Some("Audio playing.".into())));
            }
            Err(err) => {
                live.armed = false;
                live.queued = None;
                return Some(("Audio", Some(format!("Audio: {err}"))));
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
fn worker_loop(rx: std::sync::mpsc::Receiver<MixJob>, tx: std::sync::mpsc::Sender<MixDone>) {
    while let Ok(job) = rx.recv() {
        let fps = job.fps.max(1.0);
        let frame_count = (job.frames as f64 / fps * f64::from(AUDIO_RATE))
            .round()
            .max(1.0) as usize;
        let pieces = decode_chunk(&job);
        let done = MixDone {
            id: job.id,
            generation: job.generation,
            start_frame: job.start_frame,
            frame_count,
            pieces,
        };
        if tx.send(done).is_err() {
            break;
        }
    }
}

#[cfg(feature = "ffmpeg")]
fn decode_chunk(job: &MixJob) -> Result<Vec<DecodedPiece>, String> {
    let fps = job.fps.max(1.0);
    let mut any = false;
    let mut overlapped = false;
    let mut last_error = None;
    let end = job.start_frame + job.frames;
    let mut decoded_pieces = Vec::new();
    for piece in &job.pieces {
        let overlap_in = job.start_frame.max(piece.timeline_in);
        let overlap_out = end.min(piece.timeline_out);
        if overlap_out <= overlap_in {
            continue;
        }
        overlapped = true;
        let file_start =
            piece.source_at_in + (overlap_in - piece.timeline_in) as f64 * piece.seconds_per_frame;
        let file_duration = (overlap_out - overlap_in) as f64 * piece.seconds_per_frame;
        if file_duration <= 0.001 {
            continue;
        }
        let request =
            match AudioRequest::new(&piece.path, file_start.max(0.0), file_duration.min(8.0)) {
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
        let frame_offset =
            ((overlap_in - job.start_frame) as f64 / fps * f64::from(AUDIO_RATE)).round() as usize;
        decoded_pieces.push(DecodedPiece {
            clip_id: piece.clip_id,
            track_id: piece.track_id,
            samples: decoded,
            origin_frame: overlap_in as f64,
            frame_offset,
        });
    }
    if !any && last_error.is_some() && overlapped {
        return Err(last_error.unwrap_or_else(|| "audio decode failed".into()));
    }
    Ok(decoded_pieces)
}

#[cfg(feature = "ffmpeg")]
struct BusSource {
    pieces: Vec<DecodedPiece>,
    track_ids: Vec<u64>,
    acc: Vec<(u64, f32, f32)>,
    eq: Vec<(u64, EqProcessor)>,
    compressor: Vec<(u64, CompressorProcessor)>,
    control: std::sync::Arc<MixControl>,
    cached: std::sync::Arc<BusState>,
    fps: f64,
    rate: f64,
    frame_index: usize,
    frame_count: usize,
    pending_right: f32,
    expect_left: bool,
    peak: Vec<[f32; 2]>,
    sumsq: Vec<[f32; 2]>,
    track_clip: Vec<bool>,
    master_peak: [f32; 2],
    master_sumsq: [f32; 2],
    master_clip: bool,
    window: u32,
}

#[cfg(feature = "ffmpeg")]
impl BusSource {
    fn new(
        pieces: Vec<DecodedPiece>,
        frame_count: usize,
        fps: f64,
        control: std::sync::Arc<MixControl>,
    ) -> Self {
        let mut track_ids = Vec::new();
        for piece in &pieces {
            if !track_ids.contains(&piece.track_id) {
                track_ids.push(piece.track_id);
            }
        }
        let n = track_ids.len();
        let cached = control
            .params
            .lock()
            .map(|guard| std::sync::Arc::clone(&guard))
            .unwrap_or_else(|_| {
                std::sync::Arc::new(BusState {
                    master: 1.0,
                    tracks: Vec::new(),
                    clips: Vec::new(),
                })
            });
        let mut eq = Vec::new();
        let mut compressor = Vec::new();
        for id in &track_ids {
            let track = cached.tracks.iter().find(|track| track.id == *id);
            let eq_params = track.map(|track| track.eq).unwrap_or_default();
            let mut eq_processor = EqProcessor::new();
            eq_processor.set_params(eq_params);
            eq.push((*id, eq_processor));
            let comp_params = track.map(|track| track.compressor).unwrap_or_default();
            let mut comp_processor = CompressorProcessor::new();
            comp_processor.set_params(comp_params);
            compressor.push((*id, comp_processor));
        }
        Self {
            pieces,
            acc: track_ids.iter().map(|id| (*id, 0.0, 0.0)).collect(),
            track_ids,
            eq,
            compressor,
            control,
            cached,
            fps: fps.max(1.0),
            rate: f64::from(AUDIO_RATE),
            frame_index: 0,
            frame_count,
            pending_right: 0.0,
            expect_left: true,
            peak: vec![[0.0, 0.0]; n],
            sumsq: vec![[0.0, 0.0]; n],
            track_clip: vec![false; n],
            master_peak: [0.0, 0.0],
            master_sumsq: [0.0, 0.0],
            master_clip: false,
            window: 0,
        }
    }

    fn refresh(&mut self) {
        if let Ok(guard) = self.control.params.try_lock() {
            self.cached = std::sync::Arc::clone(&guard);
            for (id, processor) in &mut self.eq {
                if let Some(track) = guard.tracks.iter().find(|track| track.id == *id) {
                    processor.set_params(track.eq);
                }
            }
            for (id, processor) in &mut self.compressor {
                if let Some(track) = guard.tracks.iter().find(|track| track.id == *id) {
                    processor.set_params(track.compressor);
                }
            }
        }
    }

    fn apply_track_eq(&mut self) {
        for slot in &mut self.acc {
            if let Some((_, processor)) = self.eq.iter_mut().find(|(id, _)| *id == slot.0) {
                let (left, right) = processor.process(slot.1, slot.2);
                slot.1 = left;
                slot.2 = right;
            }
        }
    }

    fn apply_track_compressor(&mut self) {
        for slot in &mut self.acc {
            if let Some((_, processor)) = self
                .compressor
                .iter_mut()
                .find(|(id, _)| *id == slot.0)
            {
                let (left, right) = processor.process(slot.1, slot.2);
                slot.1 = left;
                slot.2 = right;
            }
        }
    }

    fn mix_at(&mut self, frame_index: usize) -> (f32, f32) {
        if frame_index % 128 == 0 {
            self.refresh();
        }
        for slot in &mut self.acc {
            slot.1 = 0.0;
            slot.2 = 0.0;
        }
        let bus = std::sync::Arc::clone(&self.cached);
        for piece in &self.pieces {
            if frame_index < piece.frame_offset {
                continue;
            }
            let index = frame_index - piece.frame_offset;
            let base = index * 2;
            if base + 1 >= piece.samples.len() {
                continue;
            }
            let sequence_frame = piece.origin_frame + index as f64 / self.rate * self.fps;
            let gain = bus.clip_gain(piece.clip_id, sequence_frame);
            let Some(slot) = self.acc.iter_mut().find(|slot| slot.0 == piece.track_id) else {
                continue;
            };
            slot.1 += piece.samples[base] * gain;
            slot.2 += piece.samples[base + 1] * gain;
        }
        self.apply_track_eq();
        self.apply_track_compressor();
        let mixed = mix_frame(&self.acc, &bus);
        self.window = self.window.saturating_add(1);
        self.master_clip |= mixed.overload;
        self.master_peak[0] = self.master_peak[0].max(mixed.left.abs());
        self.master_peak[1] = self.master_peak[1].max(mixed.right.abs());
        self.master_sumsq[0] += mixed.left * mixed.left;
        self.master_sumsq[1] += mixed.right * mixed.right;
        for index in 0..mixed.track_count as usize {
            let (id, left, right) = mixed.tracks[index];
            if let Some(slot) = self.track_ids.iter().position(|track| *track == id) {
                self.track_clip[slot] |= channel_clips(left, right);
                self.peak[slot][0] = self.peak[slot][0].max(left.abs());
                self.peak[slot][1] = self.peak[slot][1].max(right.abs());
                self.sumsq[slot][0] += left * left;
                self.sumsq[slot][1] += right * right;
            }
        }
        if self.window >= 2048 {
            self.publish();
        }
        (mixed.left, mixed.right)
    }

    fn publish(&mut self) {
        let window = self.window.max(1) as f32;
        let rms = |sum: [f32; 2]| [(sum[0] / window).sqrt(), (sum[1] / window).sqrt()];
        let mut tracks = Vec::new();
        for (index, id) in self.track_ids.iter().enumerate() {
            tracks.push((
                *id,
                self.peak[index],
                rms(self.sumsq[index]),
                self.track_clip[index],
            ));
        }
        if let Ok(params) = self.control.params.try_lock() {
            for track in &params.tracks {
                if !tracks.iter().any(|(id, _, _, _)| *id == track.id) {
                    tracks.push((track.id, [0.0, 0.0], [0.0, 0.0], false));
                }
            }
        }
        if let Ok(mut meters) = self.control.meters.try_lock() {
            meters.generation = meters.generation.saturating_add(1);
            meters.master_peak = self.master_peak;
            meters.master_rms = rms(self.master_sumsq);
            meters.master_clip = self.master_clip;
            meters.tracks = tracks;
        }
        self.window = 0;
        self.master_peak = [0.0, 0.0];
        self.master_sumsq = [0.0, 0.0];
        self.master_clip = false;
        for peak in &mut self.peak {
            *peak = [0.0, 0.0];
        }
        for sum in &mut self.sumsq {
            *sum = [0.0, 0.0];
        }
        for clip in &mut self.track_clip {
            *clip = false;
        }
    }
}

#[cfg(feature = "ffmpeg")]
impl Iterator for BusSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.frame_index >= self.frame_count && self.expect_left {
            if self.window > 0 {
                self.publish();
            }
            return None;
        }
        if self.expect_left {
            let (left, right) = self.mix_at(self.frame_index);
            self.pending_right = right;
            self.expect_left = false;
            Some(left)
        } else {
            self.expect_left = true;
            self.frame_index += 1;
            Some(self.pending_right)
        }
    }
}

#[cfg(feature = "ffmpeg")]
impl rodio::Source for BusSource {
    fn current_span_len(&self) -> Option<usize> {
        if self.frame_index >= self.frame_count {
            Some(0)
        } else {
            Some(self.frame_count.saturating_mul(2))
        }
    }

    fn channels(&self) -> std::num::NonZero<u16> {
        std::num::NonZero::new(AUDIO_CHANNELS).expect("channels")
    }

    fn sample_rate(&self) -> std::num::NonZero<u32> {
        std::num::NonZero::new(AUDIO_RATE).expect("rate")
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        Some(std::time::Duration::from_secs_f64(
            self.frame_count as f64 / self.rate,
        ))
    }
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
            proxy_path: None,
        }];
        assert!(collect_pieces(&sequence, &media, &[], 0, 48).is_empty());
        sequence.tracks[0].muted = false;
        assert!(collect_pieces(&sequence, &media, &[], 0, 48).is_empty());
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
        quiet.volume.base = 0.35;
        let mut loud = Clip::basic(5, 0, 24);
        loud.media_id = Some(MediaId(2));
        loud.volume.base = 1.5;
        sequence.tracks[0].clips = vec![quiet];
        sequence.tracks[1].clips = vec![loud];
        let media = vec![tone(MediaId(1), &keep), tone(MediaId(2), &other)];
        let mixed = collect_pieces(&sequence, &media, &[], 0, 24);
        assert_eq!(mixed.len(), 2);
        assert!((mixed[0].gain - 0.35).abs() < 1.0e-5);
        sequence.tracks[1].solo = true;
        let solo = collect_pieces(&sequence, &media, &[], 0, 24);
        assert_eq!(solo.len(), 1);
        assert!((solo[0].gain - 1.5).abs() < 1.0e-5);
        assert!(solo[0].path.ends_with("other.wav"));
        sequence.tracks[1].solo = false;
        sequence.tracks[0].fader = 2.0;
        sequence.tracks[0].pan = -0.5;
        sequence.tracks[0].muted = true;
        let bus = collect_bus_pieces(&sequence, &media, &[], 0, 24);
        assert_eq!(bus.len(), 2);
        assert!((bus[0].gain - 0.7).abs() < 1.0e-4);
        assert!((bus[0].pan + 0.5).abs() < 1.0e-5);
        assert!(collect_pieces(&sequence, &media, &[], 0, 24)
            .iter()
            .all(|piece| piece.track_id == 3));
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
            proxy_path: None,
        }
    }
}
