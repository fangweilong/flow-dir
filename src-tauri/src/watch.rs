use std::collections::HashMap;
use std::path::Path as FsPath;
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

    let handle = app.clone();
    let id = pane_id.clone();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        if result.is_ok() {
            let _ = handle.emit(
                "flowdir:dir-changed",
                DirChanged {
                    pane_id: id.clone(),
                },
            );
        }
    })
    .map_err(|e| ApiError::new("WATCH", e.to_string()))?;

    watcher
        .watch(FsPath::new(&path), RecursiveMode::NonRecursive)
        .map_err(|e| ApiError::new("WATCH", e.to_string()))?;

    map.insert(pane_id, watcher);
    Ok(())
}

pub fn clear(watchers: &Watchers, pane_id: &str) {
    watchers.0.lock().unwrap().remove(pane_id);
}
