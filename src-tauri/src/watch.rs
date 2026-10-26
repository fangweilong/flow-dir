use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Mutex;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};

use crate::error::ApiError;

#[derive(Default)]
pub struct Watchers(pub Mutex<HashMap<String, RecommendedWatcher>>);

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DirChanged {
    pub pane_id: String,
}

pub fn set<R: Runtime>(
    app: &AppHandle<R>,
    watchers: &Watchers,
    pane_id: String,
    path: String,
) -> Result<(), ApiError> {
    let mut map = watchers.0.lock().unwrap();
    map.remove(&pane_id);

    let folder = FsPath::new(&path);
    let git_dir = metadata_dir(folder);
    // Case differs when the tab path and the notify reply were produced by different
    // sources; lowercasing both keeps the event split stable on the same volume.
    let git_dir_key = git_dir.as_ref().map(|dir| dir.to_string_lossy().to_lowercase());
    let handle = app.clone();
    let id = pane_id.clone();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else {
            return;
        };
        // Index and ref writes (`git add`, `git commit`) change badges only: they must not
        // rescan a folder whose contents did not move.
        let git_only = git_dir_key.as_ref().is_some_and(|dir| {
            !event.paths.is_empty()
                && event.paths.iter().all(|path| {
                    let path = path.to_string_lossy().to_lowercase();
                    path == *dir || path.starts_with(&format!("{dir}\\"))
                })
        });
        let payload = DirChanged {
            pane_id: id.clone(),
        };
        let _ = if git_only {
            handle.emit("flowdir:git-changed", payload)
        } else {
            handle.emit("flowdir:dir-changed", payload)
        };
    })
    .map_err(|e| ApiError::new("WATCH", e.to_string()))?;

    watcher
        .watch(folder, RecursiveMode::NonRecursive)
        .map_err(|e| ApiError::new("WATCH", e.to_string()))?;
    // A pane showing a repository subfolder still needs to see `git add`/`git commit`
    // outside the app; both touch only metadata above the folder.
    if let Some(dir) = &git_dir {
        let _ = watcher.watch(dir, RecursiveMode::NonRecursive);
    }

    map.insert(pane_id, watcher);
    Ok(())
}

pub fn clear(watchers: &Watchers, pane_id: &str) {
    watchers.0.lock().unwrap().remove(pane_id);
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
