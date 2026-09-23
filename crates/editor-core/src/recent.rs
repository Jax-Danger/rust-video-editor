//! Recently opened Meridian projects.
//!
//! The list lives in `recent-projects.json` under the Meridian config directory
//! (`~/.config/meridian/`, or `MERIDIAN_CONFIG`). Newest paths come first.
//! The same file is stored once, even if it was opened through a different path.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::deliver_preset::meridian_config_dir;

/// How many projects the start window keeps.
pub const RECENT_PROJECT_LIMIT: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentProject {
    pub path: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct RecentFile {
    #[serde(default)]
    entries: Vec<RecentProject>,
}

#[derive(Debug, thiserror::Error)]
pub enum RecentError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
}

pub fn recent_projects_path() -> PathBuf {
    recent_projects_path_in(&meridian_config_dir())
}

pub fn recent_projects_path_in(config_dir: &Path) -> PathBuf {
    config_dir.join("recent-projects.json")
}

/// Load the recent list. A missing or unreadable file is an empty list.
pub fn load_recent_projects(path: &Path) -> Vec<RecentProject> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => return Vec::new(),
    };
    parse_recent_projects(&text).unwrap_or_default()
}

pub fn parse_recent_projects(text: &str) -> Result<Vec<RecentProject>, RecentError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let entries = if trimmed.starts_with('[') {
        serde_json::from_str::<Vec<RecentProject>>(trimmed)?
    } else {
        serde_json::from_str::<RecentFile>(trimmed)?.entries
    };
    Ok(normalize_recent(entries))
}

pub fn save_recent_projects(path: &Path, entries: &[RecentProject]) -> Result<(), RecentError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let file = RecentFile {
        entries: normalize_recent(entries.to_vec()),
    };
    let text = serde_json::to_string_pretty(&file)?;
    std::fs::write(path, format!("{text}\n"))?;
    Ok(())
}

/// Insert `path` at the front, drop older copies of the same file, and cap the list.
pub fn remember_recent(
    mut entries: Vec<RecentProject>,
    path: &str,
    name: &str,
) -> Vec<RecentProject> {
    let stored = canonical_project_path(path);
    if stored.is_empty() {
        return entries;
    }
    entries.retain(|entry| !same_recent_path(&entry.path, &stored));
    entries.insert(
        0,
        RecentProject {
            path: stored,
            name: name.trim().to_string(),
        },
    );
    if entries.len() > RECENT_PROJECT_LIMIT {
        entries.truncate(RECENT_PROJECT_LIMIT);
    }
    entries
}

pub fn remove_recent(mut entries: Vec<RecentProject>, path: &str) -> Vec<RecentProject> {
    entries.retain(|entry| !same_recent_path(&entry.path, path));
    entries
}

pub fn recent_project_missing(path: &str) -> bool {
    let path = path.trim();
    path.is_empty() || !Path::new(path).is_file()
}

pub fn normalize_recent(entries: Vec<RecentProject>) -> Vec<RecentProject> {
    let mut out = Vec::new();
    for entry in entries {
        let path = entry.path.trim();
        if path.is_empty() {
            continue;
        }
        if out
            .iter()
            .any(|kept: &RecentProject| same_recent_path(&kept.path, path))
        {
            continue;
        }
        out.push(RecentProject {
            path: path.to_string(),
            name: entry.name.trim().to_string(),
        });
        if out.len() == RECENT_PROJECT_LIMIT {
            break;
        }
    }
    out
}

fn canonical_project_path(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        return String::new();
    }
    match std::fs::canonicalize(path) {
        Ok(canonical) => canonical.to_string_lossy().into_owned(),
        Err(_) => path.to_string(),
    }
}

pub fn same_recent_path(a: &str, b: &str) -> bool {
    let a = a.trim();
    let b = b.trim();
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "meridian-recent-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn remember_recent_is_newest_first_and_deduped() {
        let first = remember_recent(Vec::new(), "/films/a.meridian", "A");
        let both = remember_recent(first, "/films/b.mproj", "B");
        let again = remember_recent(both, "/films/a.meridian", "A revised");
        assert_eq!(
            again
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/films/a.meridian", "/films/b.mproj"]
        );
        assert_eq!(again[0].name, "A revised");
        assert_eq!(again.len(), 2);
    }

    #[test]
    fn remember_recent_ignores_a_blank_path_and_caps_the_list() {
        let mut entries = Vec::new();
        entries = remember_recent(entries, "   ", "Nope");
        assert!(entries.is_empty());
        for index in 0..RECENT_PROJECT_LIMIT + 4 {
            entries = remember_recent(
                entries,
                &format!("/missing/project-{index}.json"),
                &format!("P{index}"),
            );
        }
        assert_eq!(entries.len(), RECENT_PROJECT_LIMIT);
        assert_eq!(
            entries[0].path,
            format!("/missing/project-{}.json", RECENT_PROJECT_LIMIT + 3)
        );
        assert_eq!(
            entries.last().unwrap().path,
            format!("/missing/project-{}.json", 4)
        );
    }

    #[test]
    fn remember_recent_collapses_paths_to_the_same_file() {
        let dir = scratch("canon");
        let file = dir.join("Cut.meridian");
        std::fs::write(&file, b"{}\n").unwrap();
        let via_dot = format!("{}/./Cut.meridian", dir.display());
        let once = remember_recent(Vec::new(), &file.to_string_lossy(), "Cut");
        let twice = remember_recent(once, &via_dot, "Cut again");
        assert_eq!(twice.len(), 1);
        assert_eq!(twice[0].name, "Cut again");
        assert_eq!(
            twice[0].path,
            file.canonicalize().unwrap().to_string_lossy()
        );
        assert!(!recent_project_missing(&twice[0].path));
        assert!(recent_project_missing("/missing/no-such.meridian"));
    }

    #[test]
    fn remove_recent_drops_only_the_matching_path() {
        let entries = remember_recent(
            remember_recent(Vec::new(), "/films/keep.meridian", "Keep"),
            "/films/drop.meridian",
            "Drop",
        );
        let left = remove_recent(entries, "/films/drop.meridian");
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].path, "/films/keep.meridian");
    }

    #[test]
    fn recent_list_roundtrips_and_rejects_junk() {
        let dir = scratch("io");
        let path = recent_projects_path_in(&dir);
        let entries = remember_recent(
            remember_recent(Vec::new(), "/films/old.json", "Old"),
            "/films/new.meridian",
            "New",
        );
        save_recent_projects(&path, &entries).unwrap();
        let loaded = load_recent_projects(&path);
        assert_eq!(loaded, entries);
        assert_eq!(load_recent_projects(&dir.join("missing.json")), Vec::new());
        assert_eq!(parse_recent_projects("").unwrap(), Vec::new());
        assert!(parse_recent_projects("{not json").is_err());
        assert_eq!(
            load_recent_projects_from_text_fallback(&dir.join("corrupt.json")),
            Vec::new()
        );

        let bare = r#"[{"path":"/films/bare.mproj","name":"Bare"},{"path":"  ","name":"skip"}]"#;
        let parsed = parse_recent_projects(bare).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "Bare");
    }

    fn load_recent_projects_from_text_fallback(path: &Path) -> Vec<RecentProject> {
        std::fs::write(path, b"{not json").unwrap();
        load_recent_projects(path)
    }

    #[test]
    fn normalize_keeps_the_front_of_an_oversized_list() {
        let entries = (0..30)
            .map(|index| RecentProject {
                path: format!("/films/{index}.meridian"),
                name: String::new(),
            })
            .collect();
        let normalized = normalize_recent(entries);
        assert_eq!(normalized.len(), RECENT_PROJECT_LIMIT);
        assert_eq!(normalized[0].path, "/films/0.meridian");
    }
}
