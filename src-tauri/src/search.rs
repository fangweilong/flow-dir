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
        if let Ok(items) = everything_search(&root, &needle, limit, hidden, &cancel) {
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
        }
        // Checked after the flush above: the batch containing the limit-th match is sent
        // first, but never with more items than the caller asked for.
        if total >= limit {
            if !batch.is_empty() {
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
            }
            break;
        }
    }

    let _ = on_batch.send(SearchBatch {
        gen,
        provider: "jwalk".into(),
        items: batch,
        done: true,
        error: None,
    });
}

#[derive(Serialize, Clone, Default)]
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

/// Builds the Everything query for a literal filename substring. Everything's own
/// operators cannot be escaped reliably, so needles containing them fall back to the
/// built-in scanner; this keeps both providers returning the same hits.
fn everything_query(root: &str, needle: &str) -> Option<String> {
    const OPERATORS: &[char] = &['"', '|', '!', '(', ')', '<', '>', '*', '?', '\\', '/'];
    if root.is_empty() || needle.chars().any(|ch| OPERATORS.contains(&ch)) {
        return None;
    }
    Some(format!(
        "path:\"{}\" \"{}\"",
        root.trim_end_matches('\\'),
        needle
    ))
}

/// Everything's `path:` is a substring filter; `C:\foo` must not match `C:\foobar`.
fn under_root(path: &str, root: &str) -> bool {
    let path = path.to_lowercase();
    let root = root.trim_end_matches('\\').to_lowercase();
    if root.is_empty() {
        return false;
    }
    path == root
        || path
            .strip_prefix(&root)
            .is_some_and(|rest| rest.starts_with('\\'))
}

/// Everything reports only the hit's own attributes; a visible file inside a hidden
/// folder must still be filtered out when hidden entries are off.
fn has_hidden_ancestor(
    path: &str,
    root: &str,
    cache: &mut std::collections::HashMap<String, bool>,
) -> bool {
    use std::os::windows::fs::MetadataExt;
    let root_key = root.trim_end_matches(['\\', '/']).to_lowercase();
    let mut current = std::path::Path::new(path).parent();
    while let Some(dir) = current {
        let key = dir.to_string_lossy().to_lowercase();
        if let Some(hidden) = cache.get(&key) {
            if *hidden {
                return true;
            }
        } else {
            let hidden = std::fs::metadata(dir)
                .map(|meta| meta.file_attributes() & (ATTR_HIDDEN | ATTR_SYSTEM) != 0)
                .unwrap_or(false);
            cache.insert(key.clone(), hidden);
            if hidden {
                return true;
            }
        }
        if key == root_key {
            break;
        }
        current = dir.parent();
    }
    false
}

fn everything_search(
    root: &str,
    needle: &str,
    limit: usize,
    hidden: bool,
    cancel: &AtomicBool,
) -> Result<Vec<SearchItem>, ApiError> {
    // Escape hatch used by the verification harness and by users who prefer the
    // built-in scanner even when Everything is running.
    if std::env::var_os("FLOWDIR_DISABLE_EVERYTHING").is_some() {
        return Err(ApiError::new(
            "EVERYTHING",
            "disabled by FLOWDIR_DISABLE_EVERYTHING",
        ));
    }
    let Some(search) = everything_query(root, needle) else {
        return Err(ApiError::new(
            "EVERYTHING",
            "query contains syntax characters; using the literal scanner",
        ));
    };
    let client =
        EverythingClient::shared().map_err(|e| ApiError::new("EVERYTHING", e.to_string()))?;
    let list = client
        .query_wait(&search)
        .request_flags(
            RequestFlags::FileName
                | RequestFlags::Path
                | RequestFlags::Size
                | RequestFlags::Attributes
                | RequestFlags::DateModified,
        )
        // Everything may return path-only matches when its "match path" option is on;
        // ask for a little more than needed so the literal name filter still fills the cap.
        .max_results(limit.saturating_mul(2).min(10_000) as u32)
        .timeout(Duration::from_millis(200))
        .call()
        .map_err(|e| ApiError::new("EVERYTHING", e.to_string()))?;

    let mut items = Vec::new();
    // Ancestor attributes are queried once per directory, not once per hit.
    let mut ancestor_cache: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    for item in list.iter() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(name) = item.get_string(RequestFlags::FileName) else {
            continue;
        };
        // Both providers match the name literally, ignore hidden/system entries the same
        // way, and treat the root as a directory boundary.
        if !name.to_lowercase().contains(needle) {
            continue;
        }
        let Some(dir) = item.get_str(RequestFlags::Path) else {
            continue;
        };
        let dir = dir.display().to_string();
        let path = format!("{}\\{}", dir.trim_end_matches('\\'), name);
        if !under_root(&path, root) {
            continue;
        }
        let attrs = item.get_u32(RequestFlags::Attributes).unwrap_or(0);
        if !hidden && attrs & (ATTR_HIDDEN | ATTR_SYSTEM) != 0 {
            continue;
        }
        if !hidden && has_hidden_ancestor(&path, root, &mut ancestor_cache) {
            continue;
        }
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
        collect_with_limit(root, query, hidden, 500)
    }

    fn collect_with_limit(root: &str, query: &str, hidden: bool, limit: u32) -> Vec<SearchItem> {
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
            limit,
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

    /// Everything's syntax characters must route to the literal scanner, and the scope
    /// filter must respect directory boundaries.
    #[test]
    fn everything_query_stays_literal_and_scoped() {
        assert_eq!(
            everything_query(r"C:\Work", "annual report").as_deref(),
            Some(r#"path:"C:\Work" "annual report""#)
        );
        assert_eq!(
            everything_query(r"C:\", "notes").as_deref(),
            Some(r#"path:"C:" "notes""#)
        );
        assert!(everything_query(r"C:\Work", "a|b").is_none());
        assert!(everything_query(r"C:\Work", "*.txt").is_none());
        assert!(everything_query("", "notes").is_none());

        assert!(under_root(r"c:\work\a.txt", r"C:\Work"));
        assert!(under_root(r"c:\work", r"C:\Work\"));
        assert!(under_root(r"\\server\share\a.txt", r"\\server\share"));
        assert!(!under_root(r"c:\workshop\a.txt", r"C:\Work"));
    }

    /// A partial final batch must be cut to the remaining quota instead of being sent whole.
    #[test]
    fn builtin_scan_never_exceeds_the_limit() {        let root = std::env::temp_dir().join(format!("flowdir-search-limit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for index in 0..130 {
            std::fs::write(root.join(format!("limit-hit-{index:03}.txt")), b"x").unwrap();
        }
        let root_text = root.to_string_lossy();
        assert_eq!(collect_with_limit(&root_text, "limit-hit", true, 120).len(), 120);
        assert_eq!(collect_with_limit(&root_text, "limit-hit", true, 49).len(), 49);
        assert_eq!(collect_with_limit(&root_text, "limit-hit", true, 1).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
