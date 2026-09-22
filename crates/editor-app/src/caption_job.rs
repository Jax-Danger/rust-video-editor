//! Background Whisper pass. The UI thread only starts the job and applies cues.

use std::sync::{Arc, Mutex};

use editor_core::{map_words_to_cues, CaptionDraft, Frame, Timebase};
use editor_media::{transcribe_wav, write_timeline_wav, WavPiece};

#[derive(Clone, Debug)]
pub struct CaptionSnapshot {
    pub finished: bool,
    pub ok: bool,
    pub drafts: Vec<CaptionDraft>,
    pub message: String,
}

pub struct CaptionJob {
    shared: Arc<Mutex<CaptionSnapshot>>,
}

impl CaptionJob {
    pub fn snapshot(&self) -> CaptionSnapshot {
        let slot = self
            .shared
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        slot.clone()
    }
}

pub fn spawn_caption(
    pieces: Vec<WavPiece>,
    range_in: i64,
    range_out: i64,
    timebase: Timebase,
    speaker: String,
    language: Option<String>,
) -> CaptionJob {
    let shared = Arc::new(Mutex::new(CaptionSnapshot {
        finished: false,
        ok: false,
        drafts: Vec::new(),
        message: "Transcribing with Whisper…".into(),
    }));
    let job = CaptionJob {
        shared: shared.clone(),
    };
    std::thread::spawn(move || {
        let result = transcribe_range(
            &pieces,
            range_in,
            range_out,
            timebase,
            &speaker,
            language.as_deref(),
        );
        let mut slot = shared.lock().unwrap_or_else(|poison| poison.into_inner());
        match result {
            Ok(drafts) => {
                let count = drafts.len();
                slot.drafts = drafts;
                slot.ok = true;
                slot.finished = true;
                slot.message = format!("Whisper wrote {count} cues.");
            }
            Err(err) => {
                slot.ok = false;
                slot.finished = true;
                slot.message = err;
            }
        }
    });
    job
}

fn transcribe_range(
    pieces: &[WavPiece],
    range_in: i64,
    range_out: i64,
    timebase: Timebase,
    speaker: &str,
    language: Option<&str>,
) -> Result<Vec<CaptionDraft>, String> {
    let dir = std::env::temp_dir().join(format!(
        "meridian-whisper-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|dur| dur.as_millis())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let wav = dir.join("caption.wav");
    let extracted = write_timeline_wav(pieces, range_in, range_out, timebase, &wav);
    let result = extracted.and_then(|_| {
        let words = transcribe_wav(&wav, language)?;
        let cues = map_words_to_cues(
            &words,
            Frame(range_in),
            Frame(range_out),
            timebase,
            Some(speaker.to_string()),
        );
        if cues.is_empty() {
            Err("Whisper returned no speech".into())
        } else {
            Ok(cues)
        }
    });
    let _ = std::fs::remove_dir_all(&dir);
    result
}
