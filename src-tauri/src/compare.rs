//! Folder comparison and the pieces of a sync preview: one directory level at a time,
//! recursive difference counts per child folder, and optional SHA-256 verification.
//!
//! Only the level the user is looking at is listed. The tree is never walked eagerly, so a
//! pair of very large folders still produces a first screen immediately; recursive counts
//! and hashes run as separately cancellable jobs.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::ipc::Channel;

use crate::error::ApiError;
use crate::hash::{self, Job, Throttle, ToolState};

const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

#[derive(Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Options {
    /// Include hidden entries; off by default, matching the file lists.
    pub include_hidden: bool,
    /// Extensions without the dot; empty means every file.
    pub extensions: Vec<String>,
    pub min_size: u64,
    pub max_size: u64,
    pub after_ms: u64,
    pub before_ms: u64,
    /// Names or simple wildcard patterns (`*`, `?`) skipped on both sides.
    pub exclude: Vec<String>,
    /// Modification times closer than this count as equal, which keeps a two-way sync
    /// from fighting over files a tool just touched on both sides.
    pub tolerance_ms: u64,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub size: u64,
    pub mtime: u64,
    pub is_dir: bool,
}

/// `status` is `left` / `right` / `different` / `same`; a type mismatch (file vs folder)
/// is reported as `different` so the user sees it without a separate category.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// Path relative to the two compare roots, with `\` separators.
    pub rel: String,
    pub name: String,
    pub left: Option<Meta>,
    pub right: Option<Meta>,
    pub status: &'static str,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Level {
    pub entries: Vec<Entry>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CountEvent {
    /// Relative path of the child folder these counts belong to.
    pub name: String,
    pub left_only: u32,
    pub right_only: u32,
    pub different: u32,
    pub same: u32,
    pub scanned: u32,
    pub done: bool,
    pub aborted: bool,
    pub error: Option<ApiError>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HashPair {
    pub rel: String,
    pub left: String,
    pub right: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HashPairResult {
    pub rel: String,
    pub equal: bool,
    pub left_hash: Option<String>,
    pub right_hash: Option<String>,
    pub error: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HashEvent {
    pub job_id: String,
    pub current: u32,
    pub total: u32,
    pub name: Option<String>,
    pub bytes: u64,
    pub bytes_total: u64,
    pub paused: bool,
    pub done: bool,
    pub aborted: bool,
    pub error: Option<ApiError>,
    pub results: Vec<HashPairResult>,
}

struct SideEntry {
    name: String,
    meta: Meta,
}

fn hidden_or_link(meta: &fs::Metadata) -> (bool, bool) {
    let attrs = meta.file_attributes();
    (
        attrs & FILE_ATTRIBUTE_HIDDEN != 0,
        attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0,
    )
}

/// `*` and `?` wildcards, case-insensitive; a pattern without wildcards matches the whole
/// name, the same way the duplicate scanner's exclusion list behaves.
fn wildcard_match(pattern: &str, name: &str) -> bool {
    let pattern = pattern.to_lowercase();
    let name = name.to_lowercase();
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = ni;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn excluded(options: &Options, name: &str) -> bool {
    options
        .exclude
        .iter()
        .any(|pattern| !pattern.is_empty() && wildcard_match(pattern, name))
}

fn extension_ok(options: &Options, name: &str) -> bool {
    if options.extensions.is_empty() {
        return true;
    }
    let lower = name.to_lowercase();
    lower
        .rsplit_once('.')
        .map(|(_, ext)| options.extensions.iter().any(|want| want == ext))
        .unwrap_or(false)
}

fn file_ok(options: &Options, name: &str, meta: &Meta) -> bool {
    if !extension_ok(options, name) {
        return false;
    }
    if options.min_size > 0 && meta.size < options.min_size {
        return false;
    }
    if options.max_size > 0 && meta.size > options.max_size {
        return false;
    }
    if options.after_ms > 0 && meta.mtime < options.after_ms {
        return false;
    }
    if options.before_ms > 0 && meta.mtime > options.before_ms {
        return false;
    }
    true
}

fn meta_of(meta: &fs::Metadata, is_dir: bool) -> Meta {
    Meta {
        size: if is_dir { 0 } else { meta.len() },
        mtime: meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        is_dir,
    }
}

/// One side of a level, keyed by the lowercased name so Windows' case-insensitive rules
/// apply; the first spelling seen is the one shown.
fn read_side(dir: &Path, options: &Options) -> Result<HashMap<String, SideEntry>, ApiError> {
    let mut out = HashMap::new();
    let reader = fs::read_dir(dir).map_err(|e| ApiError::from_io(&dir.display().to_string(), &e))?;
    for entry in reader.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(meta) = entry.metadata() else { continue };
        let (hidden, link) = hidden_or_link(&meta);
        if (hidden && !options.include_hidden) || link || excluded(options, &name) {
            continue;
        }
        let is_dir = meta.is_dir();
        let item = meta_of(&meta, is_dir);
        if !is_dir && !file_ok(options, &name, &item) {
            continue;
        }
        out.entry(name.to_lowercase()).or_insert(SideEntry { name, meta: item });
    }
    Ok(out)
}

fn join_rel(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}\\{name}")
    }
}

fn compare_maps(
    left: HashMap<String, SideEntry>,
    right: HashMap<String, SideEntry>,
    prefix: &str,
    options: &Options,
) -> Vec<Entry> {
    let mut keys: Vec<&String> = left.keys().chain(right.keys()).collect();
    keys.sort_unstable();
    keys.dedup();
    let mut entries: Vec<Entry> = keys
        .into_iter()
        .filter_map(|key| {
            let l = left.get(key);
            let r = right.get(key);
            let name = l.map(|s| s.name.clone()).or_else(|| r.map(|s| s.name.clone()))?;
            let left_meta = l.map(|s| s.meta.clone());
            let right_meta = r.map(|s| s.meta.clone());
            let status = match (&left_meta, &right_meta) {
                (Some(a), Some(b)) if a.is_dir != b.is_dir => "different",
                (Some(a), Some(b)) if a.is_dir => "same",
                (Some(a), Some(b)) => {
                    let close = a.mtime.abs_diff(b.mtime) <= options.tolerance_ms;
                    if a.size == b.size && close {
                        "same"
                    } else {
                        "different"
                    }
                }
                (Some(_), None) => "left",
                (None, Some(_)) => "right",
                (None, None) => return None,
            };
            Some(Entry {
                rel: join_rel(prefix, &name),
                name,
                left: left_meta,
                right: right_meta,
                status,
            })
        })
        .collect();
    // Folders first, then by displayed name; the same order the file lists use.
    entries.sort_by(|a, b| {
        let a_dir = a.left.as_ref().or(a.right.as_ref()).map(|m| m.is_dir).unwrap_or(false);
        let b_dir = b.left.as_ref().or(b.right.as_ref()).map(|m| m.is_dir).unwrap_or(false);
        b_dir
            .cmp(&a_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

/// Lists one level of both trees and classifies every name present on either side.
pub fn level(left: &str, right: &str, prefix: &str, options: &Options) -> Result<Level, ApiError> {
    let left_side = read_side(Path::new(left), options)?;
    let right_side = read_side(Path::new(right), options)?;
    Ok(Level {
        entries: compare_maps(left_side, right_side, prefix, options),
    })
}

/// Recursively collects files below `root` keyed by their lowercased relative path; the
/// counts only need identity, not the display spelling.
fn walk_files(
    root: &Path,
    options: &Options,
    cancel: &std::sync::atomic::AtomicBool,
    out: &mut HashMap<String, Meta>,
    scanned: &mut u32,
) {
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(reader) = fs::read_dir(&dir) else { continue };
        for entry in reader.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(meta) = entry.metadata() else { continue };
            let (hidden, link) = hidden_or_link(&meta);
            if (hidden && !options.include_hidden) || link || excluded(options, &name) {
                continue;
            }
            *scanned = scanned.saturating_add(1);
            let child_rel = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}\\{name}")
            };
            let is_dir = meta.is_dir();
            if is_dir {
                stack.push((entry.path(), child_rel));
            } else {
                let item = meta_of(&meta, false);
                if file_ok(options, &name, &item) {
                    out.insert(child_rel.to_lowercase(), item);
                }
            }
        }
    }
}

struct Counts {
    left_only: u32,
    right_only: u32,
    different: u32,
    same: u32,
}

fn count_pair(
    left: &Path,
    right: &Path,
    options: &Options,
    cancel: &std::sync::atomic::AtomicBool,
    scanned: &mut u32,
) -> Counts {
    let mut left_files = HashMap::new();
    let mut right_files = HashMap::new();
    walk_files(left, options, cancel, &mut left_files, scanned);
    if cancel.load(Ordering::Relaxed) {
        return Counts {
            left_only: 0,
            right_only: 0,
            different: 0,
            same: 0,
        };
    }
    walk_files(right, options, cancel, &mut right_files, scanned);
    let mut counts = Counts {
        left_only: 0,
        right_only: 0,
        different: 0,
        same: 0,
    };
    for (key, l) in &left_files {
        match right_files.get(key) {
            None => counts.left_only += 1,
            Some(r) if l.size == r.size && l.mtime.abs_diff(r.mtime) <= options.tolerance_ms => {
                counts.same += 1;
            }
            Some(_) => counts.different += 1,
        }
    }
    counts.right_only += right_files
        .keys()
        .filter(|key| !left_files.contains_key(*key))
        .count() as u32;
    counts
}

/// Streams one count line per shared child folder. Separate from `level` so the first
/// screen never waits for a full tree walk; the frontend can cancel at any time.
pub fn start_counts(
    state: &ToolState,
    job_id: String,
    left: String,
    right: String,
    prefix: String,
    options: Options,
    on_event: Channel<CountEvent>,
) -> Result<(), ApiError> {
    let job: Arc<Job> = state.begin(&job_id);
    let state = state.clone();
    std::thread::Builder::new()
        .name("flowdir-compare-counts".into())
        .spawn(move || {
            let root_left = Path::new(&left);
            let root_right = Path::new(&right);
            let finish = |event: CountEvent| {
                let _ = on_event.send(event);
            };
            let sides = match (read_side(root_left, &options), read_side(root_right, &options)) {
                (Ok(a), Ok(b)) => (a, b),
                (Err(error), _) | (_, Err(error)) => {
                    finish(CountEvent {
                        name: prefix.clone(),
                        left_only: 0,
                        right_only: 0,
                        different: 0,
                        same: 0,
                        scanned: 0,
                        done: true,
                        aborted: false,
                        error: Some(error),
                    });
                    state.finish(&job_id);
                    return;
                }
            };
            let mut keys: Vec<&String> = sides.0.keys().filter(|key| sides.1.contains_key(*key)).collect();
            keys.sort_unstable();
            let mut scanned = 0u32;
            for key in keys {
                let Some(l) = sides.0.get(key) else { continue };
                let Some(r) = sides.1.get(key) else { continue };
                if !l.meta.is_dir || !r.meta.is_dir {
                    continue;
                }
                if job.stopped() {
                    finish(CountEvent {
                        name: prefix.clone(),
                        left_only: 0,
                        right_only: 0,
                        different: 0,
                        same: 0,
                        scanned,
                        done: true,
                        aborted: true,
                        error: None,
                    });
                    state.finish(&job_id);
                    return;
                }
                let counts = count_pair(
                    &root_left.join(&l.name),
                    &root_right.join(&r.name),
                    &options,
                    &job.cancel,
                    &mut scanned,
                );
                finish(CountEvent {
                    name: join_rel(&prefix, &l.name),
                    left_only: counts.left_only,
                    right_only: counts.right_only,
                    different: counts.different,
                    same: counts.same,
                    scanned,
                    done: false,
                    aborted: false,
                    error: None,
                });
            }
            finish(CountEvent {
                name: prefix.clone(),
                left_only: 0,
                right_only: 0,
                different: 0,
                same: 0,
                scanned,
                done: true,
                aborted: job.stopped(),
                error: None,
            });
            state.finish(&job_id);
        })
        .map_err(|e| ApiError::new("THREAD", e.to_string()))?;
    Ok(())
}

/// SHA-256 for the pairs the caller selected (same size and close mtime). Reading is
/// sequential and honours the duplicate scanner's byte rate so a large verify does not
/// monopolise the disk.
pub fn start_hash(
    state: &ToolState,
    job_id: String,
    pairs: Vec<HashPair>,
    max_bytes_per_sec: u64,
    on_event: Channel<HashEvent>,
) -> Result<(), ApiError> {
    let job = state.begin(&job_id);
    let state = state.clone();
    let total = pairs.len().min(10_000) as u32;
    std::thread::Builder::new()
        .name("flowdir-compare-hash".into())
        .spawn(move || {
            let throttle = Throttle::new(max_bytes_per_sec);
            let bytes_total: u64 = pairs
                .iter()
                .take(total as usize)
                .map(|pair| {
                    fs::metadata(&pair.left).map(|m| m.len()).unwrap_or(0)
                        + fs::metadata(&pair.right).map(|m| m.len()).unwrap_or(0)
                })
                .sum();
            let mut results = Vec::new();
            let mut aborted = false;
            for (index, pair) in pairs.into_iter().take(total as usize).enumerate() {
                if job.stopped() {
                    aborted = true;
                    break;
                }
                let name = Path::new(&pair.left)
                    .file_name()
                    .map(|value| value.to_string_lossy().into_owned());
                let left = hash::hash_file(
                    Path::new(&pair.left),
                    "sha256",
                    &job,
                    Some(&throttle),
                    |_| {},
                );
                let right = hash::hash_file(
                    Path::new(&pair.right),
                    "sha256",
                    &job,
                    Some(&throttle),
                    |_| {},
                );
                let error = match (&left, &right) {
                    (Err(e), _) if e.code == "CANCELED" => {
                        aborted = true;
                        break;
                    }
                    (_, Err(e)) if e.code == "CANCELED" => {
                        aborted = true;
                        break;
                    }
                    (Err(e), _) => Some(e.detail.clone()),
                    (_, Err(e)) => Some(e.detail.clone()),
                    _ => None,
                };
                let equal = error.is_none()
                    && matches!((&left, &right), (Ok(a), Ok(b)) if a == b);
                let result = HashPairResult {
                    rel: pair.rel,
                    equal,
                    left_hash: left.ok(),
                    right_hash: right.ok(),
                    error,
                };
                results.push(result.clone());
                let _ = on_event.send(HashEvent {
                    job_id: job_id.clone(),
                    current: index as u32 + 1,
                    total,
                    name,
                    bytes: job.bytes.load(Ordering::Relaxed),
                    bytes_total,
                    paused: job.paused.load(Ordering::Relaxed),
                    done: index + 1 == total as usize && !aborted,
                    aborted,
                    error: None,
                    results: vec![result],
                });
            }
            let _ = on_event.send(HashEvent {
                job_id: job_id.clone(),
                current: results.len() as u32,
                total,
                name: None,
                bytes: job.bytes.load(Ordering::Relaxed),
                bytes_total,
                paused: false,
                done: true,
                aborted,
                error: None,
                results: Vec::new(),
            });
            state.finish(&job_id);
        })
        .map_err(|e| ApiError::new("THREAD", e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flowdir-compare-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn wildcards_are_case_insensitive_and_anchored() {
        assert!(wildcard_match("*.log", "App.LOG"));
        assert!(wildcard_match("node_modules", "Node_Modules"));
        assert!(!wildcard_match("*.log", "app.log.txt"));
        assert!(wildcard_match("cache?", "cache1"));
    }

    #[test]
    fn level_classifies_and_filters_both_sides() {
        let dir = scratch("level");
        let left = dir.join("left");
        let right = dir.join("right");
        write(&left.join("same.txt"), b"same");
        write(&right.join("same.txt"), b"same");
        write(&left.join("changed.txt"), b"left version");
        write(&right.join("changed.txt"), b"right");
        write(&left.join("only-left.txt"), b"l");
        write(&right.join("only-right.txt"), b"r");
        fs::create_dir_all(left.join("shared-dir")).unwrap();
        fs::create_dir_all(right.join("shared-dir")).unwrap();
        write(&left.join("skip.bin"), b"x");
        write(&right.join("skip.bin"), b"x");

        let options = Options {
            tolerance_ms: 1_000_000,
            extensions: vec!["txt".into()],
            ..Default::default()
        };
        let result = level(
            left.to_str().unwrap(),
            right.to_str().unwrap(),
            "",
            &options,
        )
        .unwrap();
        let by_name: HashMap<_, _> = result
            .entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.status))
            .collect();
        assert_eq!(by_name.get("same.txt"), Some(&"same"));
        assert_eq!(by_name.get("changed.txt"), Some(&"different"));
        assert_eq!(by_name.get("only-left.txt"), Some(&"left"));
        assert_eq!(by_name.get("only-right.txt"), Some(&"right"));
        assert_eq!(by_name.get("shared-dir"), Some(&"same"));
        assert!(!by_name.contains_key("skip.bin"));
        // Folders sort ahead of files.
        assert!(result.entries.first().unwrap().left.as_ref().unwrap().is_dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn recursive_counts_cover_nested_files() {
        let dir = scratch("counts");
        let left = dir.join("left");
        let right = dir.join("right");
        write(&left.join("shared").join("same.txt"), b"1");
        write(&right.join("shared").join("same.txt"), b"1");
        write(&left.join("shared").join("nested").join("left.txt"), b"1");
        write(&right.join("shared").join("nested").join("right.txt"), b"1");
        write(&left.join("shared").join("changed.txt"), b"longer");
        write(&right.join("shared").join("changed.txt"), b"x");
        let options = Options {
            tolerance_ms: 1_000_000,
            ..Default::default()
        };
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut scanned = 0;
        let counts = count_pair(
            &left.join("shared"),
            &right.join("shared"),
            &options,
            &cancel,
            &mut scanned,
        );
        assert_eq!(counts.same, 1);
        assert_eq!(counts.left_only, 1);
        assert_eq!(counts.right_only, 1);
        assert_eq!(counts.different, 1);
        assert!(scanned >= 4);
        let _ = fs::remove_dir_all(&dir);
    }
}