use std::os::windows::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use everything_ipc::wm::{EverythingClient, RequestFlags};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;

use crate::error::ApiError;

const BATCH: usize = 50;
const ATTR_HIDDEN: u32 = 0x2;
const ATTR_SYSTEM: u32 = 0x4;

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

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchItem {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: u64,
}

#[derive(Serialize, Deserialize, Clone)]
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
    hidden: bool,
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
    // jwalk's own `skip_hidden` only understands dot-prefixed names; Windows hides files
    // through file attributes, so the filter lives in `process_read_dir` and also prunes
    // hidden directories before they are entered.
    let walker = jwalk::WalkDir::new(&root_path)
        .skip_hidden(false)
        .follow_links(false)
        .process_read_dir(move |_depth, _path, _state, children| {
            if hidden {
                return;
            }
            children.retain(|child| {
                child
                    .as_ref()
                    .map(|entry| {
                        entry
                            .metadata()
                            .map(|meta| meta.file_attributes() & (ATTR_HIDDEN | ATTR_SYSTEM) == 0)
                            .unwrap_or(true)
                    })
                    .unwrap_or(true)
            });
        });

    let mut batch: Vec<SearchItem> = Vec::with_capacity(BATCH);
    let mut total = 0usize;
    for entry in walker {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(entry) = entry else {
            continue;
        };
        // Folders are search hits too: Everything returns them, so the built-in provider
        // must as well or the same query gives different results per provider.
        let is_dir = entry.file_type().is_dir();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.to_lowercase().contains(&needle) {
            continue;
        }
        let metadata = entry.metadata().ok();
        batch.push(SearchItem {
            path: entry.path().to_string_lossy().into_owned(),
            name,
            is_dir,
            size: if is_dir {
                0
            } else {
                metadata.as_ref().map(|m| m.len()).unwrap_or(0)
            },
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL,
        FILE_FLAGS_AND_ATTRIBUTES,
    };

    fn collect(root: &str, query: &str, hidden: bool) -> Vec<SearchItem> {
        let batches: Arc<Mutex<Vec<SearchBatch>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = batches.clone();
        let channel = Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(text) = body {
                if let Ok(batch) = serde_json::from_str::<SearchBatch>(&text) {
                    sink.lock().unwrap().push(batch);
                }
            }
            Ok(())
        });
        run(
            root.to_string(),
            query.to_string(),
            1,
            500,
            false,
            hidden,
            Arc::new(AtomicBool::new(false)),
            channel,
        );
        let batches = batches.lock().unwrap();
        batches.iter().flat_map(|batch| batch.items.clone()).collect()
    }

    /// Folders are hits in both providers, and the built-in scan follows show-hidden.
    #[test]
    fn builtin_scan_finds_folders_and_respects_hidden() {
        let root = std::env::temp_dir().join(format!("flowdir-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("report-folder")).unwrap();
        std::fs::write(root.join("report.txt"), b"x").unwrap();
        let hidden = root.join("report-hidden.txt");
        std::fs::write(&hidden, b"x").unwrap();
        let wide: Vec<u16> = hidden
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            SetFileAttributesW(
                PCWSTR(wide.as_ptr()),
                FILE_FLAGS_AND_ATTRIBUTES(FILE_ATTRIBUTE_HIDDEN.0),
            )
            .unwrap();
        }

        let items = collect(&root.to_string_lossy(), "report", false);
        let folder = items.iter().find(|item| item.name == "report-folder");
        assert!(folder.is_some_and(|item| item.is_dir && item.size == 0), "folders are hits");
        assert!(items.iter().any(|item| item.name == "report.txt"));
        assert!(!items.iter().any(|item| item.name == "report-hidden.txt"));

        let items = collect(&root.to_string_lossy(), "report", true);
        assert!(items.iter().any(|item| item.name == "report-hidden.txt"));

        unsafe {
            SetFileAttributesW(
                PCWSTR(wide.as_ptr()),
                FILE_FLAGS_AND_ATTRIBUTES(FILE_ATTRIBUTE_NORMAL.0),
            )
            .unwrap();
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
