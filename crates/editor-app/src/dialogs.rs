//! Native open, save, and import dialogs (GTK via `rfd`).

use std::path::{Path, PathBuf};

const MEDIA_EXT: &[&str] = &[
    "mp4", "mov", "mkv", "webm", "avi", "m4v", "mpg", "mpeg", "mxf", "png", "jpg", "jpeg", "tif",
    "tiff", "webp", "bmp", "gif", "exr", "dpx", "wav", "mp3", "aac", "flac", "m4a", "aiff", "aif",
    "ogg",
];

pub fn relink_media_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Relink Media")
        .add_filter("Media", MEDIA_EXT)
        .add_filter("All files", &["*"])
        .pick_file()
}

pub fn import_media_files() -> Option<Vec<PathBuf>> {
    rfd::FileDialog::new()
        .set_title("Import Media")
        .add_filter("Media", MEDIA_EXT)
        .add_filter("All files", &["*"])
        .pick_files()
}

pub fn open_project_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Open Project")
        .add_filter("Meridian project", &["json"])
        .add_filter("All files", &["*"])
        .pick_file()
}

pub fn save_project_file(suggested_name: &str, directory: Option<&Path>) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new()
        .set_title("Save Project")
        .set_file_name(suggested_name)
        .add_filter("Meridian project", &["json"]);
    if let Some(directory) = directory {
        dialog = dialog.set_directory(directory);
    }
    dialog.save_file().map(ensure_json_extension)
}

pub fn project_file_name(name: &str) -> String {
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
    let slug = if slug.is_empty() { "untitled" } else { slug };
    format!("{slug}.json")
}

fn ensure_json_extension(path: PathBuf) -> PathBuf {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("json") => path,
        _ => {
            let mut name = path.into_os_string();
            name.push(".json");
            PathBuf::from(name)
        }
    }
}

pub fn is_project_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("json")
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| !name.ends_with(".probe.json"))
}

pub fn pick_lut_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Import 3D LUT")
        .add_filter("3D LUT", &["cube"])
        .add_filter("All files", &["*"])
        .pick_file()
}
