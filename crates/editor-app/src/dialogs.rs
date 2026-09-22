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
        .add_filter("Meridian project", &["meridian", "mproj", "json"])
        .add_filter("All files", &["*"])
        .pick_file()
}

pub fn save_project_file(suggested_name: &str, directory: Option<&Path>) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new()
        .set_title("Save Project")
        .set_file_name(suggested_name)
        .add_filter("Meridian project", &["meridian"]);
    if let Some(directory) = directory {
        dialog = dialog.set_directory(directory);
    }
    dialog.save_file().map(ensure_meridian_extension)
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
    format!("{slug}.meridian")
}

fn ensure_meridian_extension(path: PathBuf) -> PathBuf {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("meridian") => path,
        _ => {
            let mut name = path.into_os_string();
            name.push(".meridian");
            PathBuf::from(name)
        }
    }
}

pub fn is_project_file(path: &Path) -> bool {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("meridian") | Some("mproj") => true,
        Some("json") => path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| !name.ends_with(".probe.json")),
        _ => false,
    }
}

pub fn pick_lut_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Import 3D LUT")
        .add_filter("3D LUT", &["cube"])
        .add_filter("All files", &["*"])
        .pick_file()
}

pub fn save_still_file(suggested_name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Export Still")
        .set_file_name(suggested_name)
        .add_filter("PNG", &["png"])
        .add_filter("JPEG", &["jpg", "jpeg"])
        .save_file()
        .map(ensure_still_extension)
}

pub fn pick_stills_folder() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Export Stills at Markers")
        .pick_folder()
}

pub fn still_file_name(sequence_name: &str, playhead: i64, marker_name: Option<&str>) -> String {
    let sequence = slug(sequence_name);
    let frame = format!("f{:04}", playhead.max(0));
    match marker_name {
        Some(name) if !name.trim().is_empty() => format!("{}_{}_{}.png", sequence, slug(name), frame),
        _ => format!("{}_{}.png", sequence, frame),
    }
}

fn slug(value: &str) -> String {
    let slug: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let slug = slug.trim_matches('_');
    if slug.is_empty() {
        "still".into()
    } else {
        slug.to_string()
    }
}

fn ensure_still_extension(path: PathBuf) -> PathBuf {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("png") | Some("jpg") | Some("jpeg") => path,
        _ => {
            let mut name = path.into_os_string();
            name.push(".png");
            PathBuf::from(name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggested_name_uses_the_meridian_extension() {
        assert_eq!(
            project_file_name("Northline Opening"),
            "Northline-Opening.meridian"
        );
        assert_eq!(project_file_name("My_Film-2"), "My_Film-2.meridian");
        assert_eq!(project_file_name("  ---  "), "untitled.meridian");
    }

    #[test]
    fn ensure_meridian_extension_appends_when_missing() {
        assert_eq!(
            ensure_meridian_extension(PathBuf::from("/films/Northline")),
            PathBuf::from("/films/Northline.meridian")
        );
        assert_eq!(
            ensure_meridian_extension(PathBuf::from("/films/Northline.meridian")),
            PathBuf::from("/films/Northline.meridian")
        );
        assert_eq!(
            ensure_meridian_extension(PathBuf::from("Northline.json")),
            PathBuf::from("Northline.json.meridian")
        );
    }

    #[test]
    fn import_rejects_project_files_and_keeps_probe_sidecars() {
        assert!(is_project_file(Path::new("Northline-Opening.meridian")));
        assert!(is_project_file(Path::new("Northline-Opening.mproj")));
        assert!(is_project_file(Path::new("samples/northline-opening.json")));
        assert!(!is_project_file(Path::new("interview.mp4")));
        assert!(!is_project_file(Path::new("interview.probe.json")));
        assert!(!is_project_file(Path::new("look.cube")));
    }
}
