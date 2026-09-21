//! Sequence presets stored as JSON under `templates/`.

use serde::{Deserialize, Serialize};

use crate::model::{Bin, BinId, Project, Sequence, SequenceId, Track, TrackId, TrackKind};
use crate::time::Timebase;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub video_tracks: u32,
    pub audio_tracks: u32,
    pub caption_tracks: u32,
}

impl ProjectTemplate {
    pub fn timebase(&self) -> Timebase {
        Timebase::new(self.fps_num, self.fps_den)
    }
}

const BUILTIN: &[&str] = &[
    include_str!("../../../templates/blank.json"),
    include_str!("../../../templates/youtube-1080p24.json"),
    include_str!("../../../templates/youtube-1080p30.json"),
    include_str!("../../../templates/vertical-9x16.json"),
    include_str!("../../../templates/cinematic-widescreen.json"),
];

pub fn builtin_templates() -> Vec<ProjectTemplate> {
    BUILTIN
        .iter()
        .map(|text| serde_json::from_str(text).expect("built-in project template is valid JSON"))
        .collect()
}

pub fn load_template_dir(path: &std::path::Path) -> Result<Vec<ProjectTemplate>, TemplateError> {
    let mut templates = Vec::new();
    let entries = std::fs::read_dir(path)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path)?;
        let template: ProjectTemplate = serde_json::from_str(&text)?;
        templates.push(template);
    }
    templates.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(templates)
}

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("template timebase is invalid")]
    InvalidTimebase,
}

pub fn project_from_template(
    template: &ProjectTemplate,
    name: &str,
) -> Result<Project, TemplateError> {
    let timebase = template.timebase();
    if !timebase.is_valid() || template.width == 0 || template.height == 0 {
        return Err(TemplateError::InvalidTimebase);
    }
    let mut project = Project::new(name);
    let bin_id = BinId(project.alloc());
    project.bins.push(Bin {
        id: bin_id,
        name: "Master".into(),
        parent: None,
    });
    let sequence_id = SequenceId(project.alloc());
    let mut sequence = Sequence::new(
        sequence_id,
        "Timeline 1",
        template.width,
        template.height,
        timebase,
    );
    push_tracks(
        &mut sequence,
        &mut project,
        TrackKind::Video,
        template.video_tracks.max(1),
    );
    push_tracks(
        &mut sequence,
        &mut project,
        TrackKind::Audio,
        template.audio_tracks.max(1),
    );
    push_tracks(
        &mut sequence,
        &mut project,
        TrackKind::Caption,
        template.caption_tracks.max(1),
    );
    project.sequences.push(sequence);
    project.active_sequence = Some(sequence_id);
    project.normalize();
    Ok(project)
}

fn push_tracks(sequence: &mut Sequence, project: &mut Project, kind: TrackKind, count: u32) {
    let prefix = kind.prefix();
    for index in 1..=count {
        let id = TrackId(project.alloc());
        sequence
            .tracks
            .push(Track::new(id, kind, format!("{prefix}{index}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_templates_cover_the_shipping_presets() {
        let templates = builtin_templates();
        assert_eq!(templates.len(), 5);
        let youtube = templates
            .iter()
            .find(|t| t.id == "youtube-1080p24")
            .unwrap();
        assert_eq!((youtube.width, youtube.height), (1920, 1080));
        assert_eq!((youtube.fps_num, youtube.fps_den), (24, 1));
        assert_eq!(youtube.video_tracks, 3);
        let vertical = templates.iter().find(|t| t.id == "vertical-9x16").unwrap();
        assert_eq!((vertical.width, vertical.height), (1080, 1920));
        let cinema = templates
            .iter()
            .find(|t| t.id == "cinematic-widescreen")
            .unwrap();
        assert_eq!((cinema.width, cinema.height), (2048, 858));
        assert_eq!((cinema.fps_num, cinema.fps_den), (24, 1));
    }

    #[test]
    fn template_builds_default_tracks() {
        let template = builtin_templates()
            .into_iter()
            .find(|t| t.id == "youtube-1080p30")
            .unwrap();
        let project = project_from_template(&template, "Launch").unwrap();
        assert_eq!(project.name, "Launch");
        let sequence = project.active().unwrap();
        assert_eq!(sequence.timebase, Timebase::fps_30());
        assert_eq!(sequence.width, 1920);
        let videos = sequence
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Video)
            .count();
        let audios = sequence
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .count();
        let captions = sequence
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Caption)
            .count();
        assert_eq!((videos, audios, captions), (3, 4, 1));
        assert_eq!(sequence.tracks[0].name, "V1");
        assert!(sequence.tracks.iter().any(|t| t.name == "C1"));
    }
}
