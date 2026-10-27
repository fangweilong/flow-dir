use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Mutex;

use notify::event::ModifyKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};

use crate::error::ApiError;

#[derive(Default)]
pub struct Watchers(pub Mutex<HashMap<String, RecommendedWatcher>>);

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DirChanged {
    pub pane_id: String,
    /// False for writes that cannot add, remove or rename a row; the listed names stay
    /// valid and re-enumerating the folder would only cost a flash.
    pub structural: bool,
}

/// Emitted when the watcher itself fails (buffer overflow, revoked access, unsupported
/// volume): the pane then falls back to a rescan instead of silently going stale.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DirError {
    pub pane_id: String,
    pub path: String,
    pub code: String,
    pub detail: String,
}

pub fn set<R: Runtime>(
    app: &AppHandle<R>,
    watchers: &Watchers,
    pane_id: String,
    path: String,
) -> Result<(), ApiError> {
    let folder = FsPath::new(&path);
    let git_dir = metadata_dir(folder);
    // Case differs when the tab path and the notify reply were produced by different
    // sources; lowercasing both keeps the event split stable on the same volume.
    let git_dir_key = git_dir.as_ref().map(|dir| dir.to_string_lossy().to_lowercase());
    // A pane showing `.git/objects` is inside the metadata directory: there, every
    // change is a normal content change, so the git-only shortcut must be disabled.
    let watched_key = path.to_lowercase();
    let inside_git = git_dir_key.as_ref().is_some_and(|dir| {
        watched_key == *dir || watched_key.starts_with(&format!("{dir}\\"))
    });
    let handle = app.clone();
    let id = pane_id.clone();
    let watched_path = path.clone();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let event = match result {
            Ok(event) => event,
            Err(err) => {
                let _ = handle.emit(
                    "flowdir:dir-error",
                    DirError {
                        pane_id: id.clone(),
                        path: watched_path.clone(),
                        code: "WATCH".into(),
                        detail: err.to_string(),
                    },
                );
                return;
            }
        };
        // Index and ref writes (`git add`, `git commit`) change badges only: they must not
        // rescan a folder whose contents did not move.
        let git_only = !inside_git
            && git_dir_key.as_ref().is_some_and(|dir| {
                !event.paths.is_empty()
                    && event.paths.iter().all(|path| {
                        let path = path.to_string_lossy().to_lowercase();
                        path == *dir || path.starts_with(&format!("{dir}\\"))
                    })
            });
        let payload = DirChanged {
            pane_id: id.clone(),
            structural: is_structural(event.kind),
        };
        let _ = if git_only {
            handle.emit("flowdir:git-changed", payload)
        } else {
            handle.emit("flowdir:dir-changed", payload)
        };
    })
    .map_err(|e| ApiError::new("WATCH", e.to_string()))?;

    // Watcher construction and `watch()` can block on a dead SMB share; only the map
    // insert is serialized, so one broken pane cannot freeze every other pane.
    watcher
        .watch(folder, RecursiveMode::NonRecursive)
        .map_err(|e| ApiError::new("WATCH", e.to_string()))?;
    // A pane showing a repository subfolder still needs to see `git add`/`git commit`
    // outside the app; both touch only metadata above the folder.
    if let Some(dir) = &git_dir {
        let _ = watcher.watch(dir, RecursiveMode::NonRecursive);
    }

    let mut map = watchers.0.lock().unwrap();
    map.insert(pane_id, watcher);
    Ok(())
}

pub fn clear(watchers: &Watchers, pane_id: &str) {
    watchers.0.lock().unwrap().remove(pane_id);
}

/// A change that can add, remove or rename a top-level row. Everything else rewrites an
/// existing entry only, so the pane keeps its rows instead of re-enumerating the folder.
///
/// Windows reports in-place writes and attribute updates as `Modify(Any)`; creations,
/// deletions and the temp-file rename used by atomic saves arrive as `Create`, `Remove`
/// and `Modify(Name)`.
fn is_structural(kind: EventKind) -> bool {
    !matches!(
        kind,
        EventKind::Modify(ModifyKind::Data(_))
            | EventKind::Modify(ModifyKind::Any)
            | EventKind::Access(_)
    )
}

/// Nearest ancestor carrying git metadata, resolving the `.git` file used by linked
/// worktrees and submodules to its actual directory.
fn metadata_dir(dir: &FsPath) -> Option<PathBuf> {
    for ancestor in dir.ancestors() {
        let dot_git = ancestor.join(".git");
        if dot_git.is_dir() {
            return Some(dot_git);
        }
        let Ok(text) = std::fs::read_to_string(&dot_git) else {
            continue;
        };
        let Some(target) = text.trim().strip_prefix("gitdir:") else {
            continue;
        };
        let target = FsPath::new(target.trim());
        let target = if target.is_absolute() {
            target.to_path_buf()
        } else {
            ancestor.join(target)
        };
        if target.is_dir() {
            return Some(target);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, DataChange, MetadataKind, RenameMode, RemoveKind};

    #[test]
    fn only_name_changes_are_structural() {
        assert!(!is_structural(EventKind::Modify(ModifyKind::Any)));
        assert!(!is_structural(EventKind::Modify(ModifyKind::Data(
            DataChange::Content
        ))));
        assert!(!is_structural(EventKind::Access(
            notify::event::AccessKind::Any
        )));
        assert!(is_structural(EventKind::Create(CreateKind::Any)));
        assert!(is_structural(EventKind::Remove(RemoveKind::Any)));
        assert!(is_structural(EventKind::Modify(ModifyKind::Name(
            RenameMode::Both
        ))));
        // Attribute changes can hide or reveal a row under the hidden-files setting.
        assert!(is_structural(EventKind::Modify(ModifyKind::Metadata(
            MetadataKind::Any
        ))));
    }

    #[test]
    fn metadata_dir_finds_the_metadata_of_a_parent_repository() {
        let root = std::env::temp_dir().join(format!("flowdir-watch-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let deep = root.join("src").join("nested");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        assert_eq!(metadata_dir(&deep), Some(root.join(".git")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn metadata_dir_resolves_a_worktree_pointer_file() {
        let root = std::env::temp_dir().join(format!("flowdir-watch-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let worktree = root.join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        let git_dir = root.join("main.git").join("worktrees").join("worktree");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .unwrap();
        assert_eq!(metadata_dir(&worktree), Some(git_dir));
        let _ = std::fs::remove_dir_all(&root);
    }
}
