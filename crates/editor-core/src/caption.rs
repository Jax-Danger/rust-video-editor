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

/// One recognised word or phrase, timed in seconds from the start of the
/// extracted audio (not the sequence).
#[derive(Clone, Debug, PartialEq)]
pub struct TimedWord {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Parse whisper.cpp JSON (`-oj` / `-ojf`) or OpenAI Whisper JSON.
///
/// Word tokens win over segment text when they carry timings. Offsets in the
/// whisper.cpp schema are milliseconds.
pub fn parse_stt_json(text: &str) -> Result<Vec<TimedWord>, CaptionError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|err| CaptionError::Failed(err.to_string()))?;
    let mut words = Vec::new();
    if let Some(segments) = value.get("transcription").and_then(|item| item.as_array()) {
        for segment in segments {
            if let Some(tokens) = segment.get("tokens").and_then(|item| item.as_array()) {
                let token_words = words_from_whisper_tokens(tokens);
                if !token_words.is_empty() {
                    words.extend(token_words);
                    continue;
                }
            }
            if let Some(word) = word_from_whisper_segment(segment) {
                words.push(word);
            }
        }
    } else if let Some(segments) = value.get("segments").and_then(|item| item.as_array()) {
        for segment in segments {
            if let Some(token_words) = segment.get("words").and_then(|item| item.as_array()) {
                let mut parsed = Vec::new();
                for token in token_words {
                    if let Some(word) = word_from_openai_word(token) {
                        parsed.push(word);
                    }
                }
                if !parsed.is_empty() {
                    words.extend(parsed);
                    continue;
                }
            }
            if let Some(word) = word_from_openai_segment(segment) {
                words.push(word);
            }
        }
    }
    if words.is_empty() {
        return Err(CaptionError::Failed(
            "speech-to-text JSON contained no timed words".into(),
        ));
    }
    Ok(words)
}

fn words_from_whisper_tokens(tokens: &[serde_json::Value]) -> Vec<TimedWord> {
    let mut words = Vec::new();
    let mut current: Option<TimedWord> = None;
    for token in tokens {
        let Some(text) = token.get("text").and_then(|item| item.as_str()) else {
            continue;
        };
        if !token_is_spoken(text) {
            continue;
        }
        let Some((start, end)) = whisper_offsets(token) else {
            continue;
        };
        let starts_word = text.starts_with(' ') || current.is_none();
        let piece = text.trim();
        if piece.is_empty() {
            continue;
        }
        if starts_word {
            if let Some(done) = current.take() {
                if spoken_text(&done.text) {
                    words.push(done);
                }
            }
            current = Some(TimedWord {
                start,
                end: end.max(start),
                text: piece.to_string(),
            });
        } else if let Some(word) = current.as_mut() {
            word.text.push_str(piece);
            word.end = end.max(word.end);
        }
    }
    if let Some(done) = current {
        if spoken_text(&done.text) {
            words.push(done);
        }
    }
    words
}

fn word_from_whisper_segment(segment: &serde_json::Value) -> Option<TimedWord> {
    let text = segment.get("text")?.as_str()?.trim();
    if !spoken_text(text) {
        return None;
    }
    let (start, end) = whisper_offsets(segment)?;
    Some(TimedWord {
        start,
        end: end.max(start + 0.01),
        text: text.to_string(),
    })
}

fn word_from_openai_word(word: &serde_json::Value) -> Option<TimedWord> {
    let text = word
        .get("word")
        .or_else(|| word.get("text"))?
        .as_str()?
        .trim();
    if !spoken_text(text) {
        return None;
    }
    let start = word.get("start")?.as_f64()?;
    let end = word.get("end")?.as_f64()?;
    Some(TimedWord {
        start,
        end: end.max(start),
        text: text.to_string(),
    })
}

fn word_from_openai_segment(segment: &serde_json::Value) -> Option<TimedWord> {
    let text = segment.get("text")?.as_str()?.trim();
    if !spoken_text(text) {
        return None;
    }
    let start = segment.get("start")?.as_f64()?;
    let end = segment.get("end")?.as_f64()?;
    Some(TimedWord {
        start,
        end: end.max(start),
        text: text.to_string(),
    })
}

fn whisper_offsets(value: &serde_json::Value) -> Option<(f64, f64)> {
    let offsets = value.get("offsets")?;
    let from = json_number(offsets.get("from")?)?;
    let to = json_number(offsets.get("to")?)?;
    Some((from / 1000.0, to / 1000.0))
}

fn json_number(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|item| item as f64))
        .or_else(|| value.as_u64().map(|item| item as f64))
}

fn token_is_spoken(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with("<|") || trimmed.starts_with("[_") || trimmed.starts_with('[') {
        return false;
    }
    true
}

fn spoken_text(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty() && token_is_spoken(trimmed)
}

/// Map words timed against a wav that spans `[timeline_in, timeline_out)` onto
/// sequence frames, then group them into editable cues.
///
/// The wav is a linear rendering of that range, including silence, so a word
/// at `t` seconds lands at `timeline_in + t * fps`.
pub fn map_words_to_cues(
    words: &[TimedWord],
    timeline_in: Frame,
    timeline_out: Frame,
    timebase: Timebase,
    speaker: Option<String>,
) -> Vec<CaptionDraft> {
    let span = timeline_out.0 - timeline_in.0;
    if span <= 0 || words.is_empty() {
        return Vec::new();
    }
    let span_secs = (span as f64 * timebase.frame_duration_secs()).max(1.0e-3);
    let to_frame = |secs: f64| -> i64 {
        let alpha = (secs / span_secs).clamp(0.0, 1.0);
        timeline_in.0 + (alpha * span as f64).round() as i64
    };
    let mut cues = Vec::new();
    let mut cue_start = 0.0;
    let mut cue_end = 0.0;
    let mut cue_text = String::new();
    let flush = |cues: &mut Vec<CaptionDraft>, start: f64, end: f64, text: &str| {
        let mut inn = to_frame(start);
        let mut out = to_frame(end.max(start));
        if out <= inn {
            out = (inn + 1).min(timeline_out.0);
        }
        inn = inn.clamp(timeline_in.0, timeline_out.0);
        out = out.clamp(timeline_in.0, timeline_out.0);
        if out <= inn || text.trim().is_empty() {
            return;
        }
        cues.push(CaptionDraft {
            timeline_in: Frame(inn),
            timeline_out: Frame(out),
            text: text.trim().to_string(),
            speaker: speaker.clone(),
        });
    };
    for word in words {
        let text = word.text.trim();
        if text.is_empty() {
            continue;
        }
        let gap = word.start - cue_end;
        let cue_secs = word.end - cue_start;
        let sentence = cue_text.ends_with('.') || cue_text.ends_with('?') || cue_text.ends_with('!');
        let too_long = !cue_text.is_empty()
            && (gap > 0.45 || cue_secs > 2.8 || cue_text.len() + text.len() > 48 || sentence);
        if too_long {
            flush(&mut cues, cue_start, cue_end, &cue_text);
            cue_text.clear();
        }
        if cue_text.is_empty() {
            cue_start = word.start;
            cue_text = text.to_string();
        } else {
            cue_text.push(' ');
            cue_text.push_str(text);
        }
        cue_end = word.end.max(cue_end);
    }
    if !cue_text.is_empty() {
        flush(&mut cues, cue_start, cue_end, &cue_text);
    }
    cues
}

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

    #[test]
    fn whisper_cpp_tokens_become_words() {
        let json = r#"{
            "transcription": [{
                "offsets": {"from": 0, "to": 1400},
                "text": " And so my",
                "tokens": [
                    {"text": "[_BEG_]", "offsets": {"from": 0, "to": 0}},
                    {"text": " And", "offsets": {"from": 0, "to": 320}},
                    {"text": " so", "offsets": {"from": 320, "to": 700}},
                    {"text": " my", "offsets": {"from": 700, "to": 1100}}
                ]
            }]
        }"#;
        let words = parse_stt_json(json).unwrap();
        assert_eq!(words.len(), 3);
        assert_eq!(words[0].text, "And");
        assert!((words[0].end - 0.32).abs() < 1e-6);
        assert_eq!(words[2].text, "my");
    }

    #[test]
    fn openai_words_and_segment_fallback() {
        let words = parse_stt_json(
            r#"{"segments":[{"start":0.0,"end":1.2,"text":"hello world","words":[
                {"word":" hello","start":0.0,"end":0.4},
                {"word":" world","start":0.5,"end":1.1}
            ]}]}"#,
        )
        .unwrap();
        assert_eq!(words.len(), 2);
        assert_eq!(words[1].text, "world");
        let segments = parse_stt_json(
            r#"{"segments":[{"start":1.0,"end":2.5,"text":" room tone under"}]}"#,
        )
        .unwrap();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "room tone under");
    }

    #[test]
    fn words_map_onto_sequence_frames_and_group() {
        let words = vec![
            TimedWord {
                start: 0.0,
                end: 0.4,
                text: "We".into(),
            },
            TimedWord {
                start: 0.4,
                end: 0.9,
                text: "came".into(),
            },
            TimedWord {
                start: 2.2,
                end: 2.8,
                text: "in.".into(),
            },
        ];
        let cues = map_words_to_cues(&words, Frame(24), Frame(24 + 96), Timebase::fps_24(), Some("INTV".into()));
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text, "We came");
        assert_eq!(cues[0].timeline_in, Frame(24));
        assert_eq!(cues[0].speaker.as_deref(), Some("INTV"));
        assert!(cues[1].timeline_in.0 > cues[0].timeline_out.0 - 1);
        assert_eq!(cues[1].text, "in.");
        assert!(cues[1].timeline_out.0 <= 24 + 96);
    }
}
