use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use everything_ipc::wm::{EverythingClient, RequestFlags};
use serde::Serialize;
use tauri::ipc::Channel;

use crate::error::ApiError;

const BATCH: usize = 50;

#[derive(Default)]
pub struct SearchState {
    current: Mutex<Option<Arc<AtomicBool>>>,
}

impl SearchState {
    pub fn begin(&self) -> Arc<AtomicBool> {
        let mut slot = self.current.lock().unwrap();
        if let Some(previous) = slot.as_ref() {
            previous.store(true, Ordering::Relaxed);
        }
        let token = Arc::new(AtomicBool::new(false));
        *slot = Some(token.clone());
        token
    }

    pub fn cancel(&self) {
        if let Some(token) = self.current.lock().unwrap().as_ref() {
            token.store(true, Ordering::Relaxed);
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchItem {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchBatch {
    pub gen: u64,
    pub provider: String,
    pub items: Vec<SearchItem>,
    pub done: bool,
    pub error: Option<ApiError>,
}

pub fn run(
    root: String,
    query: String,
    gen: u64,
    limit: u32,
    prefer_everything: bool,
    cancel: Arc<AtomicBool>,
    on_batch: Channel<SearchBatch>,
) {
    let limit = limit.clamp(1, 2000) as usize;
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        let _ = on_batch.send(SearchBatch {
            gen,
            provider: "none".into(),
            items: Vec::new(),
            done: true,
            error: None,
        });
        return;
    }

    if prefer_everything {
        if let Ok(items) = everything_search(&root, &needle, limit) {
            let _ = on_batch.send(SearchBatch {
                gen,
                provider: "everything".into(),
                items,
                done: true,
                error: None,
            });
            return;
        }
    }
    let root_path = PathBuf::from(&root);
    let walker = jwalk::WalkDir::new(&root_path)
        .skip_hidden(true)
        .follow_links(false);

    let mut batch: Vec<SearchItem> = Vec::with_capacity(BATCH);
    let mut total = 0usize;
    for entry in walker {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(entry) = entry else {
            continue;
        };
        if entry.file_type().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.to_lowercase().contains(&needle) {
            continue;
        }
        let metadata = entry.metadata().ok();
        batch.push(SearchItem {
            path: entry.path().to_string_lossy().into_owned(),
            name,
            is_dir: false,
            size: metadata.as_ref().map(|m| m.len()).unwrap_or(0),
            mtime: metadata
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        });
        total += 1;

        if batch.len() >= BATCH {
            let items = std::mem::take(&mut batch);
            if on_batch
                .send(SearchBatch {
                    gen,
                    provider: "jwalk".into(),
                    items,
                    done: false,
                    error: None,
                })
                .is_err()
            {
                return;
            }
            if total >= limit {
                break;
            }
        }
    }

    if total > limit {
        batch.truncate(limit - (total - batch.len()));
    }
    let _ = on_batch.send(SearchBatch {
        gen,
        provider: "jwalk".into(),
        items: batch,
        done: true,
        error: None,
    });
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EverythingStatus {
    pub usable: bool,
    pub version: Option<String>,
    pub reason: Option<String>,
}

/// Probes the Everything IPC channel; used by the settings page and the search
/// overlay to explain why the fast provider is or is not in use.
pub fn everything_status() -> EverythingStatus {
    if std::env::var_os("FLOWDIR_DISABLE_EVERYTHING").is_some() {
        return EverythingStatus {
            usable: false,
            version: None,
            reason: Some("disabled".into()),
        };
    }
    match EverythingClient::shared() {
        Ok(client) => {
            let version = client.get_version();
            let loaded = client.is_db_loaded();
            EverythingStatus {
                usable: loaded,
                version: Some(format!(
                    "{}.{}.{}.{}",
                    version.major, version.minor, version.revision, version.build
                )),
                reason: if loaded {
                    None
                } else {
                    Some("index-loading".into())
                },
            }
        }
        Err(err) => EverythingStatus {
            usable: false,
            version: None,
            reason: Some(err.to_string()),
        },
    }
}

fn everything_search(root: &str, needle: &str, limit: usize) -> Result<Vec<SearchItem>, ApiError> {
    // Escape hatch used by the verification harness and by users who prefer the
    // built-in scanner even when Everything is running.
    if std::env::var_os("FLOWDIR_DISABLE_EVERYTHING").is_some() {
        return Err(ApiError::new(
            "EVERYTHING",
            "disabled by FLOWDIR_DISABLE_EVERYTHING",
        ));
    }
    let client =
        EverythingClient::shared().map_err(|e| ApiError::new("EVERYTHING", e.to_string()))?;
    let search = format!("path:\"{}\" {}", root.trim_end_matches('\\'), needle);
    let list = client
        .query_wait(&search)
        .request_flags(
            RequestFlags::FileName
                | RequestFlags::Path
                | RequestFlags::Size
                | RequestFlags::Attributes
                | RequestFlags::DateModified,
        )
        .max_results(limit as u32)
        .timeout(Duration::from_millis(200))
        .call()
        .map_err(|e| ApiError::new("EVERYTHING", e.to_string()))?;

    let mut items = Vec::new();
    for item in list.iter() {
        let Some(name) = item.get_string(RequestFlags::FileName) else {
            continue;
        };
        let Some(dir) = item.get_str(RequestFlags::Path) else {
            continue;
        };
        let dir = dir.display().to_string();
        let path = format!("{}\\{}", dir.trim_end_matches('\\'), name);
        if !path.to_lowercase().starts_with(&root.to_lowercase()) {
            continue;
        }
        let attrs = item.get_u32(RequestFlags::Attributes).unwrap_or(0);
        let is_dir = attrs & 0x10 != 0;
        let mtime = item
            .get_time(RequestFlags::DateModified)
            .map(|ft| {
                let ticks = ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64;
                (ticks / 10_000 - 11_644_473_600_000).max(0) as u64
            })
            .unwrap_or(0);
        items.push(SearchItem {
            path,
            name,
            is_dir,
            size: if is_dir {
                0
            } else {
                item.get_size(RequestFlags::Size).unwrap_or(0)
            },
            mtime,
        });
        if items.len() >= limit {
            break;
        }
    }
    Ok(items)
}
