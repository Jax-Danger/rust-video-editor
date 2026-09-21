//! Background preview decode. The UI thread never waits on ffmpeg.
//!
//! One worker coalesces to the latest requested burst so a fast scrub does not
//! queue a decode per mouse move. Completed frames stay in a small cache.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use editor_media::{decode_frames, preview_backend, FrameRequest, PreviewBackend, MAX_BURST};
use egui::{ColorImage, Context, TextureHandle, TextureOptions};

const CACHE_LIMIT: usize = 96;
const LOOKAHEAD: i64 = 8;

/// Where a decode burst should start so scrubbing reuses one ffmpeg invocation.
///
/// `lead` is how many frames before `source_frame` to include. Playback passes
/// `0` (the burst runs forward from the playhead). Scrubbing and reverse pass
/// a lead so the frames under the pointer are inside the cached window.
pub fn burst_origin(source_frame: i64, last_source_frame: i64, burst: u32, lead: u32) -> (i64, u32) {
    let last = last_source_frame.max(0);
    let burst = burst.clamp(1, MAX_BURST);
    if source_frame > last {
        return (last, 1);
    }
    let start = source_frame.saturating_sub(i64::from(lead)).max(0);
    let room = (last - start + 1).max(1) as u32;
    (start, burst.min(room))
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FrameKey {
    pub path: String,
    pub source_frame: i64,
    pub width: u32,
    pub height: u32,
}

impl FrameKey {
    fn at(&self, source_frame: i64) -> Self {
        Self {
            source_frame,
            ..self.clone()
        }
    }
}

#[derive(Clone)]
pub struct PreviewImage {
    pub key: FrameKey,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

#[derive(Clone)]
pub struct PreviewQuery {
    pub path: String,
    pub source_frame: i64,
    pub width: u32,
    pub height: u32,
    pub time_secs: f64,
    pub frame_secs: f64,
    pub last_source_frame: i64,
    /// Frames per ffmpeg invocation. A short run, never one process per pixel.
    pub burst: u32,
    /// Frames of the burst that sit before [`Self::source_frame`].
    pub lead: u32,
}

impl PreviewQuery {
    fn key(&self) -> FrameKey {
        FrameKey {
            path: self.path.clone(),
            source_frame: self.source_frame,
            width: self.width,
            height: self.height,
        }
    }
}

pub enum FrameView {
    Exact(PreviewImage),
    Nearby(PreviewImage),
    Pending,
    Failed(String),
    Unavailable,
}

enum Slot {
    Pending,
    Ready(PreviewImage),
    Failed(String),
}

struct Span {
    id: u64,
    path: String,
    start: i64,
    count: u32,
    width: u32,
    height: u32,
}

struct Job {
    id: u64,
    path: String,
    start_frame: i64,
    width: u32,
    height: u32,
    time_secs: f64,
    count: u32,
}

impl Job {
    fn span(&self) -> Span {
        Span {
            id: self.id,
            path: self.path.clone(),
            start: self.start_frame,
            count: self.count,
            width: self.width,
            height: self.height,
        }
    }

    fn key(&self) -> FrameKey {
        FrameKey {
            path: self.path.clone(),
            source_frame: self.start_frame,
            width: self.width,
            height: self.height,
        }
    }
}

struct JobDone {
    id: u64,
    key: FrameKey,
    start_frame: i64,
    count: u32,
    result: Result<Vec<editor_media::DecodedFrame>, String>,
}

pub struct PreviewEngine {
    backend: PreviewBackend,
    tx: Option<Sender<Job>>,
    rx: Receiver<JobDone>,
    worker: Option<JoinHandle<()>>,
    next_id: u64,
    inflight: Option<Span>,
    queued: Option<Job>,
    slots: HashMap<FrameKey, Slot>,
    lru: VecDeque<FrameKey>,
    textures: HashMap<FrameKey, TextureHandle>,
}

impl PreviewEngine {
    pub fn new() -> Self {
        let backend = preview_backend();
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (done_tx, done_rx) = mpsc::channel::<JobDone>();
        let worker = if matches!(backend, PreviewBackend::Cli) {
            Some(thread::spawn(move || worker_loop(job_rx, done_tx)))
        } else {
            None
        };
        Self {
            backend,
            tx: worker.as_ref().map(|_| job_tx),
            rx: done_rx,
            worker,
            next_id: 1,
            inflight: None,
            queued: None,
            slots: HashMap::new(),
            lru: VecDeque::new(),
            textures: HashMap::new(),
        }
    }

    pub fn backend(&self) -> &PreviewBackend {
        &self.backend
    }

    pub fn busy(&self) -> bool {
        self.inflight.is_some() || self.queued.is_some()
    }

    pub fn request(&mut self, query: PreviewQuery) -> FrameView {
        self.poll();
        if !matches!(self.backend, PreviewBackend::Cli) {
            return FrameView::Unavailable;
        }
        let key = query.key();
        if let Some(Slot::Ready(image)) = self.slots.get(&key) {
            let image = image.clone();
            self.touch(&key);
            self.schedule_lookahead(&query);
            return FrameView::Exact(image);
        }
        if let Some(Slot::Failed(message)) = self.slots.get(&key) {
            return FrameView::Failed(message.clone());
        }
        if !self.covers(&query) {
            self.schedule(query.clone());
        }
        let slack = i64::from(query.burst.max(1)) + i64::from(query.lead);
        self.nearby(&query, slack)
            .map(FrameView::Nearby)
            .unwrap_or(FrameView::Pending)
    }

    pub fn texture(&mut self, ctx: &Context, image: &PreviewImage) -> egui::TextureId {
        if let Some(handle) = self.textures.get(&image.key) {
            return handle.id();
        }
        let color = ColorImage::from_rgba_unmultiplied(
            [image.width as usize, image.height as usize],
            &image.rgba,
        );
        let handle = ctx.load_texture(
            format!(
                "meridian-{}-{}x{}",
                image.key.source_frame, image.key.width, image.key.height
            ),
            color,
            TextureOptions::LINEAR,
        );
        let id = handle.id();
        self.textures.insert(image.key.clone(), handle);
        id
    }

    fn schedule_lookahead(&mut self, query: &PreviewQuery) {
        if query.burst <= 1 {
            return;
        }
        let ahead = query.source_frame + LOOKAHEAD;
        if ahead > query.last_source_frame {
            return;
        }
        let ahead_key = query.key().at(ahead);
        if matches!(
            self.slots.get(&ahead_key),
            Some(Slot::Ready(_) | Slot::Pending | Slot::Failed(_))
        ) {
            return;
        }
        if self.covers_frame(query, ahead) {
            return;
        }
        let mut next = query.clone();
        let delta = (ahead - query.source_frame) as f64;
        next.source_frame = ahead;
        next.time_secs = (query.time_secs + delta * query.frame_secs.max(0.0)).max(0.0);
        self.schedule(next);
    }

    fn schedule(&mut self, query: PreviewQuery) {
        let Some(tx) = self.tx.clone() else {
            return;
        };
        if query.source_frame > query.last_source_frame {
            return;
        }
        let (start_frame, count) = burst_origin(
            query.source_frame,
            query.last_source_frame,
            query.burst,
            query.lead,
        );
        let backed = (query.source_frame - start_frame).max(0) as f64;
        let time_secs = (query.time_secs - backed * query.frame_secs.max(0.0)).max(0.0);
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let job = Job {
            id,
            path: query.path,
            start_frame,
            width: query.width,
            height: query.height,
            time_secs,
            count,
        };
        if !matches!(self.slots.get(&job.key()), Some(Slot::Ready(_))) {
            self.slots.insert(job.key(), Slot::Pending);
        }
        if self.inflight.is_some() {
            if let Some(previous) = self.queued.replace(job) {
                self.forget_pending(&previous.key());
            }
            return;
        }
        self.inflight = Some(job.span());
        if tx.send(job).is_err() {
            self.inflight = None;
        }
    }

    fn covers(&self, query: &PreviewQuery) -> bool {
        self.covers_frame(query, query.source_frame)
    }

    fn covers_frame(&self, query: &PreviewQuery, frame: i64) -> bool {
        let hit = |span: &Span| {
            span.path == query.path
                && span.width == query.width
                && span.height == query.height
                && frame >= span.start
                && frame < span.start + i64::from(span.count)
        };
        self.inflight.as_ref().is_some_and(hit)
            || self
                .queued
                .as_ref()
                .map(Job::span)
                .as_ref()
                .is_some_and(hit)
    }

    fn nearby(&self, query: &PreviewQuery, max_distance: i64) -> Option<PreviewImage> {
        let mut best: Option<(i64, PreviewImage)> = None;
        for (key, slot) in &self.slots {
            if key.path != query.path || key.width != query.width || key.height != query.height {
                continue;
            }
            let Slot::Ready(image) = slot else {
                continue;
            };
            let distance = (key.source_frame - query.source_frame).abs();
            if distance > max_distance {
                continue;
            }
            if best
                .as_ref()
                .is_none_or(|(best_distance, _)| distance < *best_distance)
            {
                best = Some((distance, image.clone()));
            }
        }
        best.map(|(_, image)| image)
    }

    fn poll(&mut self) {
        loop {
            let done = match self.rx.try_recv() {
                Ok(done) => done,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.inflight = None;
                    self.queued = None;
                    break;
                }
            };
            if self
                .inflight
                .as_ref()
                .is_some_and(|span| span.id == done.id)
            {
                self.inflight = None;
            }
            match done.result {
                Ok(frames) => {
                    for (offset, frame) in frames.into_iter().enumerate() {
                        let key = done.key.at(done.start_frame + offset as i64);
                        let image = PreviewImage {
                            width: frame.width,
                            height: frame.height,
                            rgba: Arc::from(frame.rgba.into_boxed_slice()),
                            key: key.clone(),
                        };
                        self.slots.insert(key.clone(), Slot::Ready(image));
                        self.touch(&key);
                    }
                    self.evict();
                }
                Err(message) => {
                    for offset in 0..done.count {
                        let key = done.key.at(done.start_frame + i64::from(offset));
                        self.slots.insert(key, Slot::Failed(message.clone()));
                    }
                }
            }
            if self.inflight.is_none() {
                if let Some(job) = self.queued.take() {
                    self.inflight = Some(job.span());
                    if let Some(tx) = &self.tx {
                        if tx.send(job).is_err() {
                            self.inflight = None;
                        }
                    }
                }
            }
        }
    }

    fn touch(&mut self, key: &FrameKey) {
        if let Some(index) = self.lru.iter().position(|item| item == key) {
            self.lru.remove(index);
        }
        self.lru.push_back(key.clone());
    }

    fn evict(&mut self) {
        while self.lru.len() > CACHE_LIMIT {
            if let Some(old) = self.lru.pop_front() {
                self.slots.remove(&old);
                self.textures.remove(&old);
            }
        }
    }

    fn forget_pending(&mut self, key: &FrameKey) {
        if matches!(self.slots.get(key), Some(Slot::Pending)) {
            self.slots.remove(key);
        }
    }
}

impl Drop for PreviewEngine {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::burst_origin;

    #[test]
    fn scrub_burst_covers_the_playhead_without_a_job_per_frame() {
        let (start, count) = burst_origin(100, 400, 8, 3);
        assert_eq!(start, 97);
        assert_eq!(count, 8);
        assert!(100 >= start && 100 < start + i64::from(count));

        let (start, count) = burst_origin(1, 400, 8, 3);
        assert_eq!(start, 0);
        assert_eq!(count, 8);

        let (start, count) = burst_origin(398, 400, 8, 0);
        assert_eq!(start, 398);
        assert_eq!(count, 3);

        let (start, count) = burst_origin(10, 10, 8, 0);
        assert_eq!((start, count), (10, 1));
    }
}

fn worker_loop(rx: Receiver<Job>, tx: Sender<JobDone>) {
    while let Ok(job) = rx.recv() {
        let result =
            match FrameRequest::new(&job.path, job.time_secs, job.width, job.height, job.count) {
                Ok(request) => decode_frames(&request).map_err(|err| err.to_string()),
                Err(err) => Err(err.to_string()),
            };
        let done = JobDone {
            id: job.id,
            key: job.key(),
            start_frame: job.start_frame,
            count: job.count,
            result,
        };
        if tx.send(done).is_err() {
            break;
        }
    }
}
