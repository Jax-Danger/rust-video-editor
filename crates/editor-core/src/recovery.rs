//! Crash-recovery sidecars for a Meridian project.
//!
//! Autosave writes a separate JSON file. It never writes the user's project
//! path. Launch compares file modification times and only offers a recovery
//! that is strictly newer than that project (or a session that was never saved).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::deliver_preset::meridian_config_dir;
use crate::model::Project;

pub const RECOVERY_VERSION: u32 = 1;

/// Write a snapshot after this long with no further edits.
pub const AUTOSAVE_IDLE: Duration = Duration::from_secs(3);

/// Write a snapshot at this interval while edits keep arriving.
pub const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecoveryDocument {
    pub version: u32,
    /// Project file this snapshot belongs to. `None` means the user never saved.
    /// Autosave does not create or modify this path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    pub project: Project,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RecoveryIndex {
    recovery_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_path: Option<String>,
}

/// A recovery file the editor should ask the user about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryOffer {
    pub recovery_path: PathBuf,
    /// Real project path recorded in the sidecar. `None` if it was never saved.
    pub project_path: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum RecoveryError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("recovery format version {0} is not supported")]
    Version(u32),
    #[error("refusing to write recovery over the project file {0}")]
    WouldOverwriteProject(PathBuf),
    #[error("refusing to delete {0}")]
    RefusedToDelete(PathBuf),
}

/// `<dir>/<stem>.meridian/recovery.json` beside a saved project.
pub fn recovery_path_for_project(project_file: &Path) -> PathBuf {
    let parent = project_file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let stem = project_file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("project");
    let sidecar = PathBuf::from(format!("{stem}.meridian")).join("recovery.json");
    if parent == Path::new(".") {
        sidecar
    } else {
        parent.join(sidecar)
    }
}

/// Never-saved sessions land here, not next to a project the user did not choose.
pub fn unsaved_recovery_path() -> PathBuf {
    unsaved_recovery_path_in(&meridian_config_dir())
}

pub fn unsaved_recovery_path_in(config_dir: &Path) -> PathBuf {
    config_dir.join("recovery").join("unsaved.json")
}

pub fn recovery_index_path() -> PathBuf {
    meridian_config_dir().join("recovery-index.json")
}

pub fn recovery_path(project_file: Option<&Path>) -> PathBuf {
    match project_file {
        Some(path) => recovery_path_for_project(path),
        None => unsaved_recovery_path(),
    }
}

/// True for sidecar files this module is allowed to create and delete.
pub fn is_sidecar_recovery_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(parent) = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
    else {
        return false;
    };
    if name == "recovery.json" && parent.ends_with(".meridian") {
        return true;
    }
    parent == "recovery" && name == "unsaved.json"
}

/// `recovery` is usable when it is strictly newer than the project file.
/// A missing project file counts as older: the session was never saved.
pub fn recovery_is_newer(recovery_mtime: SystemTime, project_mtime: Option<SystemTime>) -> bool {
    match project_mtime {
        Some(project_mtime) => recovery_mtime > project_mtime,
        None => true,
    }
}

/// Whether a dirty session should write its sidecar now.
///
/// `since_last_edit` is the idle clock (it resets on every edit).
/// `unwritten_for` is how long the current unsaved generation has been waiting
/// (it does not reset while typing continues).
pub fn autosave_due(
    dirty: bool,
    generation: u64,
    written_generation: u64,
    since_last_edit: Duration,
    unwritten_for: Duration,
    idle: Duration,
    interval: Duration,
) -> bool {
    if !dirty || generation == written_generation {
        return false;
    }
    since_last_edit >= idle || unwritten_for >= interval
}

/// How long to sleep before checking [`autosave_due`] again.
pub fn autosave_delay(
    since_last_edit: Duration,
    unwritten_for: Duration,
    idle: Duration,
    interval: Duration,
) -> Duration {
    let until_idle = idle.saturating_sub(since_last_edit);
    let until_interval = interval.saturating_sub(unwritten_for);
    let delay = until_idle.min(until_interval);
    if delay.is_zero() {
        Duration::from_millis(250)
    } else {
        delay
    }
}

pub fn load_recovery(path: &Path) -> Result<RecoveryDocument, RecoveryError> {
    let text = std::fs::read_to_string(path)?;
    let doc: RecoveryDocument = serde_json::from_str(&text)?;
    if doc.version != RECOVERY_VERSION {
        return Err(RecoveryError::Version(doc.version));
    }
    Ok(doc)
}

/// Write the sidecar and point the launch index at it.
///
/// `project_path` is recorded inside the document and is not created or modified.
pub fn publish_recovery(
    project_path: Option<&Path>,
    project: &Project,
) -> Result<PathBuf, RecoveryError> {
    let recovery = recovery_path(project_path);
    publish_recovery_to(&recovery, &recovery_index_path(), project_path, project)?;
    Ok(recovery)
}

pub(crate) fn publish_recovery_to(
    recovery_path: &Path,
    index_path: &Path,
    project_path: Option<&Path>,
    project: &Project,
) -> Result<(), RecoveryError> {
    if let Some(project_path) = project_path {
        if same_path(project_path, recovery_path) {
            return Err(RecoveryError::WouldOverwriteProject(
                project_path.to_path_buf(),
            ));
        }
    }
    let doc = RecoveryDocument {
        version: RECOVERY_VERSION,
        project_path: project_path.map(|path| path.to_string_lossy().into_owned()),
        project: project.clone(),
    };
    write_atomic(recovery_path, &serde_json::to_string_pretty(&doc)?)?;
    let index = RecoveryIndex {
        recovery_path: recovery_path.to_string_lossy().into_owned(),
        project_path: project_path.map(|path| path.to_string_lossy().into_owned()),
    };
    write_atomic(index_path, &serde_json::to_string_pretty(&index)?)?;
    Ok(())
}

/// Delete a sidecar and the launch index when the index points at it.
/// Project files are refused.
pub fn forget_recovery(recovery_path: &Path, index_path: &Path) -> Result<(), RecoveryError> {
    if !is_sidecar_recovery_path(recovery_path) {
        return Err(RecoveryError::RefusedToDelete(recovery_path.to_path_buf()));
    }
    if recovery_path.is_file() {
        std::fs::remove_file(recovery_path)?;
    }
    let partial = partial_path(recovery_path);
    if partial.is_file() {
        let _ = std::fs::remove_file(partial);
    }
    clear_index_if_points_at(index_path, recovery_path)?;
    Ok(())
}

pub fn launch_recovery_offer() -> Option<RecoveryOffer> {
    launch_offer_at(&recovery_index_path(), &unsaved_recovery_path())
}

fn launch_offer_at(index_path: &Path, unsaved_path: &Path) -> Option<RecoveryOffer> {
    if index_path.is_file() {
        return offer_from_index_file(index_path);
    }
    offer_if_newer(unsaved_path, None)
}

pub fn offer_for_project_file(project_file: &Path) -> Option<RecoveryOffer> {
    offer_if_newer(&recovery_path_for_project(project_file), Some(project_file))
}

fn offer_from_index_file(index_path: &Path) -> Option<RecoveryOffer> {
    let text = std::fs::read_to_string(index_path).ok()?;
    let index: RecoveryIndex = serde_json::from_str(&text).ok()?;
    let recovery = PathBuf::from(&index.recovery_path);
    let project = index.project_path.as_deref().map(Path::new);
    offer_if_newer(&recovery, project)
}

fn offer_if_newer(recovery: &Path, expected_project: Option<&Path>) -> Option<RecoveryOffer> {
    if !recovery.is_file() {
        return None;
    }
    if let Some(project) = expected_project {
        if same_path(project, recovery) {
            return None;
        }
    }
    let recovery_mtime = std::fs::metadata(recovery).ok()?.modified().ok()?;
    let project_mtime = expected_project.and_then(|path| {
        if path.is_file() {
            std::fs::metadata(path).ok()?.modified().ok()
        } else {
            None
        }
    });
    if !recovery_is_newer(recovery_mtime, project_mtime) {
        return None;
    }
    let doc = load_recovery(recovery).ok()?;
    match (doc.project_path.as_deref(), expected_project) {
        (Some(recorded), Some(expected)) if same_path(Path::new(recorded), expected) => {}
        (None, None) => {}
        _ => return None,
    }
    Some(RecoveryOffer {
        recovery_path: recovery.to_path_buf(),
        project_path: doc.project_path.map(PathBuf::from),
    })
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn partial_path(path: &Path) -> PathBuf {
    let mut partial = path.as_os_str().to_os_string();
    partial.push(".partial");
    PathBuf::from(partial)
}

fn write_atomic(path: &Path, body: &str) -> Result<(), RecoveryError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let partial = partial_path(path);
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&partial)?;
        file.write_all(body.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    std::fs::rename(&partial, path)?;
    Ok(())
}

fn clear_index_if_points_at(index_path: &Path, recovery_path: &Path) -> Result<(), RecoveryError> {
    if !index_path.is_file() {
        return Ok(());
    }
    let text = match std::fs::read_to_string(index_path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    let Ok(index) = serde_json::from_str::<RecoveryIndex>(&text) else {
        return Ok(());
    };
    if same_path(Path::new(&index.recovery_path), recovery_path) {
        std::fs::remove_file(index_path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "meridian-recovery-{name}-{}-{}",
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

    fn set_mtime(path: &Path, secs: u64) {
        let file = OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    fn sample_project(name: &str) -> Project {
        Project::new(name)
    }

    #[test]
    fn saved_project_recovery_is_a_sidecar() {
        let project = Path::new("/films/northline.json");
        let recovery = recovery_path_for_project(project);
        assert_eq!(
            recovery,
            PathBuf::from("/films/northline.meridian/recovery.json")
        );
        assert_ne!(recovery, project);
        assert!(is_sidecar_recovery_path(&recovery));
        assert!(!is_sidecar_recovery_path(project));
        assert!(!is_sidecar_recovery_path(Path::new("/films/recovery.json")));
        assert!(!is_sidecar_recovery_path(Path::new(
            "/films/northline.meridian/proxies/clip.mp4"
        )));
    }

    #[test]
    fn unsaved_recovery_lives_under_the_config_dir() {
        let path = unsaved_recovery_path_in(Path::new("/cfg/meridian"));
        assert_eq!(path, PathBuf::from("/cfg/meridian/recovery/unsaved.json"));
        assert!(is_sidecar_recovery_path(&path));
        assert_ne!(path, Path::new("/cfg/meridian/project.json"));
    }

    #[test]
    fn relative_project_still_uses_a_sidecar_directory() {
        let recovery = recovery_path_for_project(Path::new("notes.json"));
        assert_eq!(recovery, PathBuf::from("notes.meridian/recovery.json"));
        assert_ne!(recovery, Path::new("notes.json"));
    }

    #[test]
    fn newer_mtime_is_required() {
        let older = UNIX_EPOCH + Duration::from_secs(10);
        let newer = UNIX_EPOCH + Duration::from_secs(20);
        assert!(recovery_is_newer(newer, Some(older)));
        assert!(!recovery_is_newer(older, Some(newer)));
        assert!(!recovery_is_newer(newer, Some(newer)));
        assert!(recovery_is_newer(older, None));
    }

    #[test]
    fn autosave_waits_for_idle_or_the_interval() {
        let idle = AUTOSAVE_IDLE;
        let interval = AUTOSAVE_INTERVAL;
        assert!(!autosave_due(false, 2, 1, idle, interval, idle, interval));
        assert!(!autosave_due(true, 4, 4, idle, interval, idle, interval));
        assert!(autosave_due(
            true,
            2,
            1,
            Duration::from_secs(3),
            Duration::from_secs(3),
            idle,
            interval
        ));
        assert!(autosave_due(
            true,
            2,
            1,
            Duration::from_millis(100),
            Duration::from_secs(60),
            idle,
            interval
        ));
        assert!(!autosave_due(
            true,
            2,
            1,
            Duration::from_millis(100),
            Duration::from_secs(10),
            idle,
            interval
        ));
        assert!(!autosave_due(
            true,
            2,
            1,
            Duration::from_millis(2999),
            Duration::from_secs(10),
            idle,
            interval
        ));
        assert_eq!(
            autosave_delay(
                Duration::from_secs(1),
                Duration::from_secs(10),
                idle,
                interval
            ),
            Duration::from_secs(2)
        );
        assert_eq!(
            autosave_delay(
                Duration::from_secs(1),
                Duration::from_secs(59),
                idle,
                interval
            ),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn publish_writes_the_sidecar_and_leaves_the_project_file_alone() {
        let dir = scratch("publish");
        let project_path = dir.join("film.json");
        std::fs::write(&project_path, b"ORIGINAL").unwrap();
        let recovery = recovery_path_for_project(&project_path);
        let index = dir.join("recovery-index.json");
        let project = sample_project("Northline");

        publish_recovery_to(&recovery, &index, Some(&project_path), &project).unwrap();

        assert_eq!(std::fs::read(&project_path).unwrap(), b"ORIGINAL");
        assert!(recovery.is_file());
        assert!(!partial_path(&recovery).exists());
        let doc = load_recovery(&recovery).unwrap();
        assert_eq!(doc.version, RECOVERY_VERSION);
        assert_eq!(doc.project.name, "Northline");
        assert_eq!(
            doc.project_path.as_deref(),
            Some(project_path.to_str().unwrap())
        );
        let err = Project::from_json(&std::fs::read_to_string(&recovery).unwrap());
        assert!(err.is_err());

        set_mtime(&project_path, 1_000);
        set_mtime(&recovery, 2_000);
        let offer = offer_from_index_file(&index).unwrap();
        assert_eq!(offer.recovery_path, recovery);
        assert_eq!(offer.project_path.as_deref(), Some(project_path.as_path()));
        assert_eq!(std::fs::read(&project_path).unwrap(), b"ORIGINAL");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_or_equal_recovery_is_not_offered() {
        let dir = scratch("stale");
        let project_path = dir.join("film.json");
        std::fs::write(&project_path, b"SAVED").unwrap();
        let recovery = recovery_path_for_project(&project_path);
        let index = dir.join("recovery-index.json");
        publish_recovery_to(
            &recovery,
            &index,
            Some(&project_path),
            &sample_project("Northline"),
        )
        .unwrap();

        set_mtime(&recovery, 1_000);
        set_mtime(&project_path, 1_000);
        assert!(offer_from_index_file(&index).is_none());
        assert!(offer_for_project_file(&project_path).is_none());

        set_mtime(&project_path, 2_000);
        assert!(offer_from_index_file(&index).is_none());
        assert_eq!(std::fs::read(&project_path).unwrap(), b"SAVED");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_project_file_still_offers_recovery() {
        let dir = scratch("missing");
        let project_path = dir.join("missing.json");
        let recovery = recovery_path_for_project(&project_path);
        let index = dir.join("recovery-index.json");
        publish_recovery_to(
            &recovery,
            &index,
            Some(&project_path),
            &sample_project("Draft"),
        )
        .unwrap();
        assert!(!project_path.exists());
        let offer = offer_from_index_file(&index).unwrap();
        assert_eq!(offer.project_path.as_deref(), Some(project_path.as_path()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unsaved_launch_offer_uses_the_slot_when_there_is_no_index() {
        let dir = scratch("unsaved");
        let unsaved = unsaved_recovery_path_in(&dir);
        let index = dir.join("recovery-index.json");
        publish_recovery_to(&unsaved, &index, None, &sample_project("Untitled")).unwrap();
        std::fs::remove_file(&index).unwrap();

        let offer = launch_offer_at(&index, &unsaved).unwrap();
        assert!(offer.project_path.is_none());
        assert_eq!(offer.recovery_path, unsaved);

        let stale_index = dir.join("stale-index.json");
        publish_recovery_to(&unsaved, &stale_index, None, &sample_project("Untitled")).unwrap();
        let project = dir.join("later.json");
        std::fs::write(&project, b"LATER").unwrap();
        let project_recovery = recovery_path_for_project(&project);
        publish_recovery_to(
            &project_recovery,
            &stale_index,
            Some(&project),
            &sample_project("Later"),
        )
        .unwrap();
        set_mtime(&project_recovery, 1_000);
        set_mtime(&project, 5_000);
        assert!(launch_offer_at(&stale_index, &unsaved).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn partial_file_and_mismatched_project_are_ignored() {
        let dir = scratch("partial");
        let project_path = dir.join("film.json");
        std::fs::write(&project_path, b"ORIGINAL").unwrap();
        let recovery = recovery_path_for_project(&project_path);
        std::fs::create_dir_all(recovery.parent().unwrap()).unwrap();
        std::fs::write(partial_path(&recovery), b"{}\n").unwrap();
        let index = dir.join("recovery-index.json");
        std::fs::write(
            &index,
            serde_json::json!({
                "recovery_path": recovery,
                "project_path": project_path,
            })
            .to_string(),
        )
        .unwrap();
        assert!(offer_from_index_file(&index).is_none());

        publish_recovery_to(
            &recovery,
            &index,
            Some(&project_path),
            &sample_project("Northline"),
        )
        .unwrap();
        let other = dir.join("other.json");
        std::fs::write(&other, b"OTHER").unwrap();
        set_mtime(&recovery, 3_000);
        set_mtime(&other, 1_000);
        let wrong_index = dir.join("wrong-index.json");
        std::fs::write(
            &wrong_index,
            serde_json::json!({
                "recovery_path": recovery,
                "project_path": other,
            })
            .to_string(),
        )
        .unwrap();
        assert!(offer_from_index_file(&wrong_index).is_none());
        assert_eq!(std::fs::read(&project_path).unwrap(), b"ORIGINAL");
        assert_eq!(std::fs::read(&other).unwrap(), b"OTHER");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn publish_refuses_to_use_the_project_path_as_the_sidecar() {
        let dir = scratch("collide");
        let project_path = dir.join("film.json");
        std::fs::write(&project_path, b"ORIGINAL").unwrap();
        let index = dir.join("recovery-index.json");
        let err = publish_recovery_to(
            &project_path,
            &index,
            Some(&project_path),
            &sample_project("Northline"),
        )
        .unwrap_err();
        assert!(matches!(err, RecoveryError::WouldOverwriteProject(_)));
        assert_eq!(std::fs::read(&project_path).unwrap(), b"ORIGINAL");
        assert!(!index.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn forget_removes_the_sidecar_and_index_only() {
        let dir = scratch("forget");
        let project_path = dir.join("film.json");
        std::fs::write(&project_path, b"ORIGINAL").unwrap();
        let recovery = recovery_path_for_project(&project_path);
        let index = dir.join("recovery-index.json");
        publish_recovery_to(
            &recovery,
            &index,
            Some(&project_path),
            &sample_project("Northline"),
        )
        .unwrap();
        std::fs::write(partial_path(&recovery), b"partial").unwrap();

        forget_recovery(&recovery, &index).unwrap();
        assert!(!recovery.exists());
        assert!(!partial_path(&recovery).exists());
        assert!(!index.exists());
        assert_eq!(std::fs::read(&project_path).unwrap(), b"ORIGINAL");

        let err = forget_recovery(&project_path, &index).unwrap_err();
        assert!(matches!(err, RecoveryError::RefusedToDelete(_)));
        assert_eq!(std::fs::read(&project_path).unwrap(), b"ORIGINAL");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
