//! Deliver / export presets stored as JSON under `presets/deliver/`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliverPreset {
    pub id: String,
    pub name: String,
    pub description: String,
    pub codec: String,
    pub container: String,
    pub filename_suffix: String,
    pub burn_captions: bool,
    pub use_in_out: bool,
    #[serde(default)]
    pub video_bitrate_kbps: Option<u32>,
    #[serde(default)]
    pub audio_bitrate_kbps: Option<u32>,
    #[serde(default)]
    pub audio_only: bool,
}

/// Session settings mirrored in the Deliver panel and persisted between launches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliverSettings {
    pub preset_id: String,
    pub codec: String,
    pub container: String,
    pub use_in_out: bool,
    pub burn_captions: bool,
    pub output_path: String,
    #[serde(default)]
    pub video_bitrate_kbps: Option<u32>,
    #[serde(default)]
    pub audio_bitrate_kbps: Option<u32>,
    #[serde(default)]
    pub audio_only: bool,
}

impl Default for DeliverSettings {
    fn default() -> Self {
        let preset = default_deliver_preset();
        let container = preset.container.clone();
        Self {
            preset_id: preset.id.clone(),
            codec: preset.codec,
            container: container.clone(),
            use_in_out: preset.use_in_out,
            burn_captions: preset.burn_captions,
            output_path: default_output_path(&container),
            video_bitrate_kbps: preset.video_bitrate_kbps,
            audio_bitrate_kbps: preset.audio_bitrate_kbps,
            audio_only: preset.audio_only,
        }
    }
}

impl DeliverSettings {
    pub fn from_preset(preset: &DeliverPreset, sequence_name: &str, current_output: &str) -> Self {
        Self {
            preset_id: preset.id.clone(),
            codec: preset.codec.clone(),
            container: preset.container.clone(),
            use_in_out: preset.use_in_out,
            burn_captions: preset.burn_captions,
            output_path: suggest_output_path(current_output, sequence_name, preset),
            video_bitrate_kbps: preset.video_bitrate_kbps,
            audio_bitrate_kbps: preset.audio_bitrate_kbps,
            audio_only: preset.audio_only,
        }
    }
}

const BUILTIN: &[&str] = &[
    include_str!("../../../presets/deliver/youtube-1080p.json"),
    include_str!("../../../presets/deliver/youtube-shorts.json"),
    include_str!("../../../presets/deliver/instagram.json"),
    include_str!("../../../presets/deliver/master-prores.json"),
    include_str!("../../../presets/deliver/h265-smaller.json"),
    include_str!("../../../presets/deliver/audio-wav.json"),
];

pub fn builtin_deliver_presets() -> Vec<DeliverPreset> {
    BUILTIN
        .iter()
        .map(|text| serde_json::from_str(text).expect("built-in deliver preset is valid JSON"))
        .collect()
}

pub fn default_deliver_preset() -> DeliverPreset {
    builtin_deliver_presets()
        .into_iter()
        .find(|preset| preset.id == "youtube-1080p")
        .unwrap_or_else(|| builtin_deliver_presets().into_iter().next().unwrap())
}

pub fn load_deliver_preset_dir(path: &Path) -> Result<Vec<DeliverPreset>, DeliverPresetError> {
    let mut presets = Vec::new();
    if !path.is_dir() {
        return Ok(presets);
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path)?;
        let preset: DeliverPreset = serde_json::from_str(&text)?;
        presets.push(preset);
    }
    presets.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(presets)
}

pub fn save_deliver_preset(path: &Path, preset: &DeliverPreset) -> Result<(), DeliverPresetError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(preset)?;
    std::fs::write(path, text)?;
    Ok(())
}

pub fn all_deliver_presets(custom_dir: Option<&Path>) -> Vec<DeliverPreset> {
    let mut presets = builtin_deliver_presets();
    if let Some(dir) = custom_dir {
        if let Ok(custom) = load_deliver_preset_dir(dir) {
            for preset in custom {
                if let Some(slot) = presets.iter_mut().find(|existing| existing.id == preset.id) {
                    *slot = preset;
                } else {
                    presets.push(preset);
                }
            }
        }
    }
    presets.sort_by(|a, b| a.name.cmp(&b.name));
    presets
}

pub fn suggest_output_path(
    current_output: &str,
    sequence_name: &str,
    preset: &DeliverPreset,
) -> String {
    let parent = Path::new(current_output)
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("/tmp"));
    let slug = slugify(sequence_name);
    let ext = container_extension(&preset.container);
    parent
        .join(format!("{slug}{}{ext}", preset.filename_suffix))
        .to_string_lossy()
        .into_owned()
}

pub fn default_output_path(container: &str) -> String {
    format!("/tmp/meridian-export{}", container_extension(container))
}

pub fn container_extension(container: &str) -> &'static str {
    match container.trim().to_ascii_lowercase().as_str() {
        "mov" => ".mov",
        "mxf" => ".mxf",
        "wav" => ".wav",
        _ => ".mp4",
    }
}

pub fn meridian_config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MERIDIAN_CONFIG") {
        return PathBuf::from(dir);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config/meridian");
    }
    std::env::temp_dir().join("meridian")
}

pub fn deliver_last_settings_path() -> PathBuf {
    meridian_config_dir().join("deliver-last.json")
}

pub fn custom_deliver_preset_dir() -> PathBuf {
    meridian_config_dir().join("presets/deliver")
}

pub fn load_last_deliver_settings() -> Option<DeliverSettings> {
    let path = deliver_last_settings_path();
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_last_deliver_settings(settings: &DeliverSettings) -> Result<(), DeliverPresetError> {
    let path = deliver_last_settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(settings)?;
    std::fs::write(path, text)?;
    Ok(())
}

fn slugify(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "export".into()
    } else {
        slug.to_string()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeliverPresetError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_presets_cover_shipping_profiles() {
        let presets = builtin_deliver_presets();
        assert_eq!(presets.len(), 6);
        let youtube = presets.iter().find(|p| p.id == "youtube-1080p").unwrap();
        assert_eq!(youtube.codec, "H.264");
        assert_eq!(youtube.container, "mp4");
        assert_eq!(youtube.filename_suffix, "_youtube");
        let audio = presets.iter().find(|p| p.id == "audio-wav").unwrap();
        assert!(audio.audio_only);
        assert_eq!(audio.container, "wav");
    }

    #[test]
    fn applying_preset_fills_deliver_fields() {
        let preset = builtin_deliver_presets()
            .into_iter()
            .find(|p| p.id == "h265-smaller")
            .unwrap();
        let settings = DeliverSettings::from_preset(
            &preset,
            "Northline Opening",
            "/tmp/meridian-export.mp4",
        );
        assert_eq!(settings.codec, "H.265");
        assert_eq!(settings.container, "mp4");
        assert_eq!(settings.preset_id, "h265-smaller");
        assert!(settings.burn_captions);
        assert!(!settings.use_in_out);
        assert_eq!(settings.audio_bitrate_kbps, Some(160));
        assert!(settings.output_path.ends_with("_h265.mp4"));
        assert!(settings.output_path.contains("Northline-Opening"));
    }

    #[test]
    fn master_preset_targets_prores_mov() {
        let preset = builtin_deliver_presets()
            .into_iter()
            .find(|p| p.id == "master-prores")
            .unwrap();
        let settings = DeliverSettings::from_preset(&preset, "Timeline 1", "/tmp/out.mp4");
        assert_eq!(settings.codec, "ProRes 422");
        assert_eq!(settings.container, "mov");
        assert!(!settings.burn_captions);
        assert!(settings.output_path.ends_with("_master.mov"));
    }

    #[test]
    fn last_settings_round_trip() {
        let dir = std::env::temp_dir().join(format!("meridian-deliver-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("deliver-last.json");
        let settings = DeliverSettings {
            preset_id: "instagram".into(),
            codec: "H.264".into(),
            container: "mp4".into(),
            use_in_out: true,
            burn_captions: false,
            output_path: "/tmp/test_instagram.mp4".into(),
            video_bitrate_kbps: None,
            audio_bitrate_kbps: Some(128),
            audio_only: false,
        };
        let text = serde_json::to_string_pretty(&settings).unwrap();
        std::fs::write(&path, text).unwrap();
        let loaded: DeliverSettings = serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
            .unwrap();
        assert_eq!(loaded, settings);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn custom_preset_dir_loads_json() {
        let dir = std::env::temp_dir().join(format!("meridian-presets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let preset = DeliverPreset {
            id: "my-web".into(),
            name: "My Web Export".into(),
            description: "Custom web preset".into(),
            codec: "H.264".into(),
            container: "mp4".into(),
            filename_suffix: "_web".into(),
            burn_captions: true,
            use_in_out: false,
            video_bitrate_kbps: Some(8000),
            audio_bitrate_kbps: Some(192),
            audio_only: false,
        };
        save_deliver_preset(&dir.join("my-web.json"), &preset).unwrap();
        let loaded = load_deliver_preset_dir(&dir).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "my-web");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
