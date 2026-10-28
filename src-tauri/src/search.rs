use std::io::Read;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use everything_ipc::wm::{EverythingClient, RequestFlags};
use regex::{Regex, RegexBuilder};
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
    /// Line that matched a content search; `None` for name-only hits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

/// Attribute/size/time filters combined with the query. Empty values mean "no bound".
#[derive(Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchFilter {
    pub min_size: u64,
    pub max_size: u64,
    /// Milliseconds since the Unix epoch.
    pub after_ms: u64,
    pub before_ms: u64,
    /// Lower-case extensions without the dot; empty means any type.
    pub extensions: Vec<String>,
    /// `file`, `dir`, or empty for both.
    pub kind: String,
}

impl SearchFilter {
    pub fn is_empty(&self) -> bool {
        self.min_size == 0
            && self.max_size == 0
            && self.after_ms == 0
            && self.before_ms == 0
            && self.extensions.is_empty()
            && self.kind.is_empty()
    }

    fn matches(&self, is_dir: bool, size: u64, mtime: u64, name: &str) -> bool {
        match self.kind.as_str() {
            "file" if is_dir => return false,
            "dir" if !is_dir => return false,
            _ => {}
        }
        let is_file = !is_dir;
        if is_file && self.min_size > 0 && size < self.min_size {
            return false;
        }
        if is_file && self.max_size > 0 && size > self.max_size {
            return false;
        }
        if self.after_ms > 0 && mtime < self.after_ms {
            return false;
        }
        if self.before_ms > 0 && mtime > self.before_ms {
            return false;
        }
        if !self.extensions.is_empty() {
            if is_dir {
                return false;
            }
            let ext = name.rsplit_once('.').map(|(_, ext)| ext.to_lowercase());
            match ext {
                Some(ext) if self.extensions.iter().any(|want| *want == ext) => {}
                _ => return false,
            }
        }
        true
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct ContentOptions {
    pub enabled: bool,
    pub regex: bool,
    /// Files larger than this are not scanned for content.
    pub max_bytes: u64,
}

impl Default for ContentOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            regex: false,
            max_bytes: 10 * 1024 * 1024,
        }
    }
}

/// Extensions cheap and meaningful to scan as text. Anything else is left to the name
/// search; probing arbitrary binaries would add I/O without useful snippets.
const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "log", "json", "jsonl", "js", "mjs", "cjs", "ts", "tsx", "jsx",
    "css", "scss", "less", "html", "htm", "xml", "yaml", "yml", "toml", "ini", "cfg", "conf",
    "csv", "tsv", "rs", "py", "java", "kt", "c", "h", "cpp", "hpp", "cc", "go", "sh", "bash",
    "bat", "cmd", "ps1", "sql", "vue", "svelte", "gradle", "properties", "env", "gitignore",
    "editorconfig", "lock",
];

fn is_text_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    TEXT_EXTENSIONS.contains(&ext)
}

/// Reads at most `max_bytes`, rejects binary content by its first NUL byte, and returns
/// the line around the first match. `None` means "no match or unreadable".
fn content_snippet(path: &Path, regex: &Regex, max_bytes: u64, cancel: &AtomicBool) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(max_bytes).read_to_end(&mut bytes).ok()?;
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return None;
    }
    let text = String::from_utf8_lossy(&bytes);
    let found = regex.find(&text)?;
    let start = text[..found.start()].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = text[found.end()..]
        .find('\n')
        .map(|i| found.end() + i)
        .unwrap_or(text.len());
    let line = text[start..end].trim();
    // A minified single-line file would otherwise carry megabytes into the row.
    let snippet: String = line.chars().take(240).collect();
    Some(snippet)
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
    filter: SearchFilter,
    content: ContentOptions,
    cancel: Arc<AtomicBool>,
    on_batch: Channel<SearchBatch>,
) {
    // 5000 rows is the documented ceiling; Everything is asked for a little more than the
    // literal filter needs, so this clamp also bounds that request.
    let limit = limit.clamp(1, 5000) as usize;
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

    // Content matching is always regex-based: a literal query is escaped, so both modes
    // share one matcher and the snippet offsets stay valid for the original text.
    let content_re: Option<Regex> = if content.enabled {
        let pattern = if content.regex {
            query.trim().to_string()
        } else {
            regex::escape(query.trim())
        };
        match RegexBuilder::new(&pattern).case_insensitive(true).build() {
            Ok(regex) => Some(regex),
            Err(err) => {
                let _ = on_batch.send(SearchBatch {
                    gen,
                    provider: "none".into(),
                    items: Vec::new(),
                    done: true,
                    error: Some(ApiError::new("BAD_REGEX", err.to_string())),
                });
                return;
            }
        }
    } else {
        None
    };
    let content_bytes = content.max_bytes.clamp(1024, 512 * 1024 * 1024);

    // Everything cannot express size/time/attribute filters or read file contents; the
    // built-in scanner is slower but is the only provider that can honour those requests.
    if prefer_everything && filter.is_empty() && content_re.is_none() {
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
        let metadata = entry.metadata().ok();
        let size = if is_dir {
            0
        } else {
            metadata.as_ref().map(|m| m.len()).unwrap_or(0)
        };
        let mtime = metadata
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if !filter.matches(is_dir, size, mtime, &name) {
            continue;
        }
        let name_hit = name.to_lowercase().contains(&needle);
        let snippet = if name_hit {
            None
        } else {
            let Some(regex) = content_re.as_ref() else {
                continue;
            };
            // Binary and oversized files are never read for content; the extension list
            // keeps the scan away from media and executables that would only cost I/O.
            if is_dir || size > content_bytes || !is_text_name(&name) {
                continue;
            }
            let path = entry.path();
            match content_snippet(&path, regex, content_bytes, &cancel) {
                Some(found) => Some(found),
                None => continue,
            }
        };
        batch.push(SearchItem {
            path: entry.path().to_string_lossy().into_owned(),
            name,
            is_dir,
            size,
            mtime,
            snippet,
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
            snippet: None,
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
        collect_full(root, query, hidden, limit, SearchFilter::default(), ContentOptions::default())
    }

    fn collect_full(
        root: &str,
        query: &str,
        hidden: bool,
        limit: u32,
        filter: SearchFilter,
        content: ContentOptions,
    ) -> Vec<SearchItem> {
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
            filter,
            content,
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

    #[test]
    fn filters_and_content_search_narrow_the_result() {
        let root = std::env::temp_dir().join(format!("flowdir-search-filter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("big.log"), vec![b'x'; 4096]).unwrap();
        std::fs::write(root.join("small.log"), b"x").unwrap();
        std::fs::write(root.join("big.txt"), vec![b'x'; 4096]).unwrap();
        std::fs::write(root.join("notes.md"), b"line one\nhere is the NEEDLE inside\nline three\n").unwrap();
        // A binary file whose bytes happen to contain the query must not match content.
        std::fs::write(root.join("blob.txt"), b"\0\x01needle\0").unwrap();

        let filter = SearchFilter {
            min_size: 1024,
            extensions: vec!["log".into()],
            ..SearchFilter::default()
        };
        let hits = collect_full(&root.to_string_lossy(), "", false, 500, filter, ContentOptions::default());
        assert!(hits.is_empty(), "an empty query returns nothing even with filters");

        let filter = SearchFilter {
            min_size: 1024,
            extensions: vec!["log".into()],
            ..SearchFilter::default()
        };
        let hits = collect_full(&root.to_string_lossy(), "big", false, 500, filter, ContentOptions::default());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "big.log");

        let content = ContentOptions {
            enabled: true,
            regex: false,
            max_bytes: 1024 * 1024,
        };
        let hits = collect_full(&root.to_string_lossy(), "needle", false, 500, SearchFilter::default(), content);
        let notes = hits.iter().find(|hit| hit.name == "notes.md").expect("content hit");
        assert!(notes.snippet.as_deref().unwrap_or("").to_lowercase().contains("needle"));
        assert!(!hits.iter().any(|hit| hit.name == "blob.txt"), "binary files are skipped");

        let content = ContentOptions {
            enabled: true,
            regex: true,
            max_bytes: 1024 * 1024,
        };
        let hits = collect_full(&root.to_string_lossy(), r"NEEDLE\s+inside", false, 500, SearchFilter::default(), content);
        assert!(hits.iter().any(|hit| hit.name == "notes.md"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
