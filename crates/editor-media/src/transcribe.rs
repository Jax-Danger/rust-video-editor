//! Local speech-to-text by shelling out to whisper.cpp's `whisper-cli`.
//!
//! Nothing here downloads a model or links whisper. [`whisper_availability`]
//! reports a missing binary or model so the app can keep the stub transcriber.

use std::path::{Path, PathBuf};
use std::process::Command;

use editor_core::{parse_stt_json, TimedWord};

#[derive(Clone, Debug)]
pub struct WhisperPaths {
    pub binary: PathBuf,
    pub model: PathBuf,
}

pub fn whisper_availability() -> Result<WhisperPaths, String> {
    let binary = find_binary()?;
    let model = find_model()?;
    Ok(WhisperPaths { binary, model })
}

/// Transcribe a 16 kHz mono wav. Word timings come from whisper.cpp `-ojf`
/// tokens when the model emits them, otherwise from segment offsets.
pub fn transcribe_wav(wav: &Path, language: Option<&str>) -> Result<Vec<TimedWord>, String> {
    let paths = whisper_availability()?;
    if !wav.is_file() {
        return Err(format!("caption audio is missing ({})", wav.display()));
    }
    let dir = wav
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    let out_base = dir.join("whisper-out");
    let dtw = dtw_preset(&paths.model);
    let mut attempt = run_whisper(
        &paths.binary,
        &paths.model,
        wav,
        language,
        &out_base,
        dtw.as_deref(),
    );
    if attempt.is_err() && dtw.is_some() {
        let _ = std::fs::remove_file(out_base.with_extension("json"));
        attempt = run_whisper(&paths.binary, &paths.model, wav, language, &out_base, None);
    }
    let json_path = attempt?;
    let text = std::fs::read_to_string(&json_path).map_err(|err| err.to_string())?;
    parse_stt_json(&text).map_err(|err| err.to_string())
}

fn run_whisper(
    binary: &Path,
    model: &Path,
    wav: &Path,
    language: Option<&str>,
    out_base: &Path,
    dtw: Option<&str>,
) -> Result<PathBuf, String> {
    let _ = std::fs::remove_file(out_base.with_extension("json"));
    let mut command = Command::new(binary);
    command
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(wav)
        .arg("-ojf")
        .arg("-of")
        .arg(out_base)
        .arg("-np")
        .arg("-t")
        .arg(thread_count());
    if let Some(language) = language.filter(|value| !value.trim().is_empty()) {
        command.arg("-l").arg(language);
    }
    if let Some(dtw) = dtw {
        command.arg("-dtw").arg(dtw);
    }
    let output = command.output().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            format!("whisper-cli was not found at {}", binary.display())
        } else {
            err.to_string()
        }
    })?;
    let json_path = out_base.with_extension("json");
    if !output.status.success() || !json_path.is_file() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = stderr
            .lines()
            .chain(stdout.lines())
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(6)
            .collect::<Vec<_>>()
            .join(" ");
        return Err(if detail.is_empty() {
            format!("whisper-cli exited with {}", output.status)
        } else {
            format!("whisper-cli failed: {detail}")
        });
    }
    Ok(json_path)
}

fn thread_count() -> String {
    std::thread::available_parallelism()
        .map(|count| count.get().clamp(1, 4).to_string())
        .unwrap_or_else(|_| "2".into())
}

fn dtw_preset(model: &Path) -> Option<String> {
    let name = model.file_name()?.to_str()?;
    let name = name.strip_prefix("ggml-").unwrap_or(name);
    let name = name.strip_suffix(".bin").unwrap_or(name);
    let preset = name.replace('-', ".");
    const KNOWN: &[&str] = &[
        "tiny",
        "tiny.en",
        "base",
        "base.en",
        "small",
        "small.en",
        "medium",
        "medium.en",
        "large.v1",
        "large.v2",
        "large.v3",
        "large.v3.turbo",
    ];
    KNOWN.iter().any(|item| *item == preset).then_some(preset)
}

fn find_binary() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("WHISPER_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "WHISPER_BIN is set but {} is not a file",
            path.display()
        ));
    }
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            candidates.push(dir.join("whisper-cli"));
            candidates.push(dir.join("whisper-cpp"));
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(home.join(".local/bin/whisper-cli"));
        candidates.push(home.join(".cache/meridian/whisper.cpp/build/bin/whisper-cli"));
    }
    candidates.push(PathBuf::from("/usr/local/bin/whisper-cli"));
    candidates.push(PathBuf::from("/usr/bin/whisper-cli"));
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            "whisper-cli is not on PATH. Install whisper.cpp and set WHISPER_BIN, or see the README."
                .into()
        })
}

fn find_model() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("WHISPER_MODEL") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "WHISPER_MODEL is set but {} is not a file",
            path.display()
        ));
    }
    let mut candidates = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let cache = PathBuf::from(home).join(".cache/whisper");
        for name in [
            "ggml-tiny.en.bin",
            "ggml-base.en.bin",
            "ggml-tiny.bin",
            "ggml-base.bin",
            "ggml-small.en.bin",
        ] {
            candidates.push(cache.join(name));
        }
    }
    candidates.push(PathBuf::from("models/ggml-tiny.en.bin"));
    candidates.push(PathBuf::from("models/ggml-tiny.bin"));
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            "no Whisper ggml model found. Download ggml-tiny.en.bin into ~/.cache/whisper or set WHISPER_MODEL."
                .into()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    #[test]
    fn dtw_preset_matches_whisper_cpp_names() {
        assert_eq!(
            dtw_preset(Path::new("/models/ggml-tiny.en.bin")).as_deref(),
            Some("tiny.en")
        );
        assert_eq!(
            dtw_preset(Path::new("ggml-large-v3.bin")).as_deref(),
            Some("large.v3")
        );
        assert_eq!(dtw_preset(Path::new("custom.bin")), None);
    }

    #[test]
    fn missing_binary_is_a_clear_error() {
        let _guard = env_lock();
        let unique = format!("/tmp/meridian-no-whisper-{}", std::process::id());
        std::env::set_var("WHISPER_BIN", &unique);
        let err = find_binary().unwrap_err();
        assert!(err.contains("WHISPER_BIN"), "{err}");
        std::env::remove_var("WHISPER_BIN");
    }

    #[test]
    fn transcribes_speech_when_whisper_cli_is_installed() {
        let _guard = env_lock();
        if whisper_availability().is_err() {
            return;
        }
        if Command::new("espeak-ng").arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("meridian-stt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("voice.wav");
        let status = Command::new("espeak-ng")
            .args(["-s", "130", "-w"])
            .arg(&wav)
            .arg("We came in over the ridge at first light.")
            .status()
            .expect("espeak-ng");
        assert!(status.success());
        let words = transcribe_wav(&wav, Some("en")).expect("whisper");
        let joined = words
            .iter()
            .map(|word| word.text.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            joined.contains("ridge") || joined.contains("came"),
            "{joined}"
        );
        assert!(words.iter().all(|word| word.end >= word.start));
        let cues = editor_core::map_words_to_cues(
            &words,
            editor_core::Frame(24),
            editor_core::Frame(24 + (5 * 24)),
            editor_core::Timebase::fps_24(),
            Some("Voice".into()),
        );
        assert!(!cues.is_empty());
        assert!(cues[0].timeline_in.0 >= 24);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
