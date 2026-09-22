//! Bounded disk cache of decoded preview frames.
//!
//! Keys are `(asset path, source frame, pixel size, source mtime)`. A scrub
//! that revisits the same frames reads the files instead of spawning ffmpeg
//! again. The budget is a target: one frame larger than the budget is kept,
//! and older files are removed until the rest fits.

use std::path::{Path, PathBuf};

use crate::decode::DecodedFrame;

/// Default on-disk preview cache. About 250 frames of 960×540 RGBA, or many
/// more at the program monitor's smaller sizes.
pub const DEFAULT_FRAME_CACHE_BYTES: u64 = 512 * 1024 * 1024;

/// Stop adding files after this many, even when they are tiny.
pub const DEFAULT_FRAME_CACHE_FILES: usize = 4_096;

const MAGIC: &[u8; 4] = b"MFR1";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FrameCacheKey {
    pub path: String,
    pub source_frame: i64,
    pub width: u32,
    pub height: u32,
    /// `mtime` seconds of the source. A replaced file misses the old entry.
    pub stamp: u64,
}

impl FrameCacheKey {
    pub fn new(path: &str, source_frame: i64, width: u32, height: u32) -> Self {
        Self {
            path: path.to_string(),
            source_frame,
            width,
            height,
            stamp: source_stamp(path),
        }
    }
}

struct Entry {
    name: String,
    bytes: u64,
    tick: u64,
}

pub struct FrameCache {
    dir: PathBuf,
    max_bytes: u64,
    max_files: usize,
    entries: Vec<Entry>,
    used: u64,
    clock: u64,
    enabled: bool,
}

impl FrameCache {
    pub fn open(dir: impl Into<PathBuf>, max_bytes: u64) -> Self {
        Self::open_limited(dir, max_bytes, DEFAULT_FRAME_CACHE_FILES)
    }

    pub fn open_limited(dir: impl Into<PathBuf>, max_bytes: u64, max_files: usize) -> Self {
        let dir = dir.into();
        let enabled = std::fs::create_dir_all(&dir).is_ok();
        let mut cache = Self {
            dir,
            max_bytes: max_bytes.max(1),
            max_files: max_files.max(1),
            entries: Vec::new(),
            used: 0,
            clock: 1,
            enabled,
        };
        if cache.enabled {
            cache.reload();
        }
        cache
    }

    pub fn bytes_used(&self) -> u64 {
        self.used
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&mut self, key: &FrameCacheKey) -> Option<DecodedFrame> {
        let frame = self.read_file(key)?;
        self.touch(key);
        Some(frame)
    }

    pub fn get_burst(
        &mut self,
        path: &str,
        start_frame: i64,
        count: u32,
        width: u32,
        height: u32,
    ) -> Option<Vec<DecodedFrame>> {
        if count == 0 {
            return None;
        }
        let stamp = source_stamp(path);
        let mut frames = Vec::with_capacity(count as usize);
        for offset in 0..count {
            let key = keyed(path, start_frame + i64::from(offset), width, height, stamp);
            frames.push(self.read_file(&key)?);
        }
        for offset in 0..count {
            let key = keyed(path, start_frame + i64::from(offset), width, height, stamp);
            self.touch(&key);
        }
        Some(frames)
    }

    pub fn put(&mut self, key: &FrameCacheKey, frame: &DecodedFrame) {
        if !self.enabled || frame.rgba.is_empty() {
            return;
        }
        let name = file_name(key);
        let path = self.dir.join(&name);
        let bytes = encode(frame);
        if std::fs::write(&path, &bytes).is_err() {
            return;
        }
        let len = bytes.len() as u64;
        let tick = self.bump();
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.name == name) {
            self.used = self.used.saturating_sub(entry.bytes);
            entry.bytes = len;
            entry.tick = tick;
            self.used = self.used.saturating_add(len);
        } else {
            self.entries.push(Entry {
                name,
                bytes: len,
                tick,
            });
            self.used = self.used.saturating_add(len);
        }
        self.evict();
    }

    pub fn put_burst(
        &mut self,
        path: &str,
        start_frame: i64,
        width: u32,
        height: u32,
        frames: &[DecodedFrame],
    ) {
        let stamp = source_stamp(path);
        for (offset, frame) in frames.iter().enumerate() {
            let key = keyed(path, start_frame + offset as i64, width, height, stamp);
            self.put(&key, frame);
        }
    }

    fn read_file(&self, key: &FrameCacheKey) -> Option<DecodedFrame> {
        if !self.enabled {
            return None;
        }
        let bytes = std::fs::read(self.dir.join(file_name(key))).ok()?;
        decode_stored(&bytes, key.width, key.height)
    }

    fn touch(&mut self, key: &FrameCacheKey) {
        let name = file_name(key);
        let tick = self.bump();
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.name == name) {
            entry.tick = tick;
        }
    }

    fn bump(&mut self) -> u64 {
        self.clock = self.clock.saturating_add(1);
        self.clock
    }

    fn reload(&mut self) {
        self.entries.clear();
        self.used = 0;
        let Ok(dir) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for item in dir.flatten() {
            let name = item.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.ends_with(".mfr") {
                continue;
            }
            let Ok(meta) = item.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let tick = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|time| time.as_secs())
                .unwrap_or(0);
            self.clock = self.clock.max(tick);
            self.used = self.used.saturating_add(meta.len());
            self.entries.push(Entry {
                name: name.to_string(),
                bytes: meta.len(),
                tick,
            });
        }
        self.evict();
    }

    fn evict(&mut self) {
        while self.entries.len() > 1
            && (self.used > self.max_bytes || self.entries.len() > self.max_files)
        {
            let Some((index, _)) = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.tick)
            else {
                break;
            };
            let entry = self.entries.remove(index);
            self.used = self.used.saturating_sub(entry.bytes);
            let _ = std::fs::remove_file(self.dir.join(entry.name));
        }
    }
}

fn keyed(path: &str, source_frame: i64, width: u32, height: u32, stamp: u64) -> FrameCacheKey {
    FrameCacheKey {
        path: path.to_string(),
        source_frame,
        width,
        height,
        stamp,
    }
}

pub fn source_stamp(path: &str) -> u64 {
    std::fs::metadata(Path::new(path))
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|time| time.as_secs())
        .unwrap_or(0)
}

fn file_name(key: &FrameCacheKey) -> String {
    format!(
        "{:016x}-{:x}-{}x{}-{}.mfr",
        fnv1a(key.path.as_bytes()),
        key.source_frame as u64,
        key.width,
        key.height,
        key.stamp
    )
}

fn encode(frame: &DecodedFrame) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(12 + frame.rgba.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&frame.width.to_le_bytes());
    bytes.extend_from_slice(&frame.height.to_le_bytes());
    bytes.extend_from_slice(&frame.rgba);
    bytes
}

fn decode_stored(bytes: &[u8], width: u32, height: u32) -> Option<DecodedFrame> {
    if bytes.len() < 12 || &bytes[..4] != MAGIC {
        return None;
    }
    let stored_w = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    let stored_h = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    if stored_w != width || stored_h != height {
        return None;
    }
    let rgba = bytes[12..].to_vec();
    let expected = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    if rgba.len() != expected {
        return None;
    }
    Some(DecodedFrame {
        width,
        height,
        rgba,
    })
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

    fn frame(fill: u8) -> DecodedFrame {
        DecodedFrame {
            width: 2,
            height: 2,
            rgba: vec![fill; 16],
        }
    }

    #[test]
    fn roundtrip_is_keyed_by_asset_frame_and_size_and_evicts() {
        let dir = std::env::temp_dir().join(format!("meridian-frames-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut cache = FrameCache::open_limited(&dir, 80, 8);
        let path = dir.join("source.mp4");
        std::fs::write(&path, b"src").unwrap();
        let path = path.to_string_lossy().into_owned();

        let first = FrameCacheKey::new(&path, 0, 2, 2);
        cache.put(&first, &frame(1));
        let loaded = cache.get(&first).unwrap();
        assert_eq!(loaded.rgba, vec![1; 16]);
        assert!(cache.get(&FrameCacheKey::new(&path, 1, 2, 2)).is_none());
        assert!(cache.get(&FrameCacheKey::new(&path, 0, 4, 2)).is_none());

        cache.put(&FrameCacheKey::new(&path, 1, 2, 2), &frame(2));
        cache.put(&FrameCacheKey::new(&path, 2, 2, 2), &frame(3));
        assert!(
            cache.bytes_used() <= 80 || cache.len() == 1,
            "used {} count {}",
            cache.bytes_used(),
            cache.len()
        );
        assert!(cache.len() < 3, "oldest frame should have been evicted");
        assert!(cache.get(&FrameCacheKey::new(&path, 2, 2, 2)).is_some());

        let again = FrameCache::open_limited(&dir, 80, 8);
        assert!(!again.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn burst_hits_only_when_every_frame_is_present() {
        let dir = std::env::temp_dir().join(format!("meridian-burst-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut cache = FrameCache::open(&dir, DEFAULT_FRAME_CACHE_BYTES);
        let path = dir.join("clip.mp4");
        std::fs::write(&path, b"clip").unwrap();
        let path = path.to_string_lossy().into_owned();
        assert!(cache.get_burst(&path, 4, 2, 2, 2).is_none());
        cache.put_burst(&path, 4, 2, 2, &[frame(9), frame(8)]);
        let burst = cache.get_burst(&path, 4, 2, 2, 2).unwrap();
        assert_eq!(burst.len(), 2);
        assert_eq!(burst[0].rgba[0], 9);
        assert_eq!(burst[1].rgba[0], 8);
        assert!(cache.get_burst(&path, 4, 3, 2, 2).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
