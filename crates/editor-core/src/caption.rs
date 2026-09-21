//! Caption cues and a pluggable transcriber.
//!
//! [`StubTranscriber`] needs no network and no API key. Swap in a Whisper,
//! whisper.cpp, or cloud speech-to-text adapter by implementing
//! [`CaptionTranscriber`]. See the repository README.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::time::{Frame, Timebase};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptionDraft {
    pub timeline_in: Frame,
    pub timeline_out: Frame,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscribeRequest {
    pub media_name: String,
    pub language: Option<String>,
    pub range_in: Frame,
    pub range_out: Frame,
    pub timebase: Timebase,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CaptionError {
    #[error("nothing to transcribe")]
    EmptyRange,
    #[error("transcriber failed: {0}")]
    Failed(String),
}

pub trait CaptionTranscriber {
    fn transcribe(
        &mut self,
        request: &TranscribeRequest,
    ) -> Result<Vec<CaptionDraft>, CaptionError>;
}

/// Deterministic mock. Splits the requested range into fixed-length cues and
/// cycles a small phrase list. Replace this type to call a real STT engine.
#[derive(Clone, Debug)]
pub struct StubTranscriber {
    /// Target cue length in seconds. Converted with the request timebase.
    pub cue_seconds: f64,
}

impl Default for StubTranscriber {
    fn default() -> Self {
        Self { cue_seconds: 2.0 }
    }
}

const STUB_LINES: &[&str] = &[
    "We came in over the ridge at first light.",
    "Hold the skyline — leave room for the caption.",
    "The city was already awake.",
    "Cut on the look, not on the line.",
    "Room tone under the pause.",
];

impl CaptionTranscriber for StubTranscriber {
    fn transcribe(
        &mut self,
        request: &TranscribeRequest,
    ) -> Result<Vec<CaptionDraft>, CaptionError> {
        let span = request.range_out.0 - request.range_in.0;
        if span <= 0 {
            return Err(CaptionError::EmptyRange);
        }
        let fps = request.timebase.fps_f64().max(1.0);
        let cue_len = ((self.cue_seconds.max(0.25) * fps).round() as i64).max(1);
        let mut cues = Vec::new();
        let mut cursor = request.range_in.0;
        let mut index = 0;
        while cursor < request.range_out.0 {
            let end = (cursor + cue_len).min(request.range_out.0);
            let line = STUB_LINES[index % STUB_LINES.len()];
            cues.push(CaptionDraft {
                timeline_in: Frame(cursor),
                timeline_out: Frame(end),
                text: line.to_string(),
                speaker: Some(request.media_name.clone()),
            });
            cursor = end;
            index += 1;
            if index > 10_000 {
                break;
            }
        }
        Ok(cues)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_transcriber_is_deterministic() {
        let mut t = StubTranscriber::default();
        let request = TranscribeRequest {
            media_name: "interview.mov".into(),
            language: Some("en".into()),
            range_in: Frame(0),
            range_out: Frame(96),
            timebase: Timebase::fps_24(),
        };
        let a = t.transcribe(&request).unwrap();
        let b = t.transcribe(&request).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].timeline_in, Frame(0));
        assert_eq!(a[0].timeline_out, Frame(48));
        assert_eq!(a[1].timeline_in, Frame(48));
        assert_eq!(a[1].timeline_out, Frame(96));
        assert_eq!(a[0].text, STUB_LINES[0]);
        assert_eq!(a[0].speaker.as_deref(), Some("interview.mov"));
    }
}
