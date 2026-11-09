//! Archive extraction: format detection, per-format backends and the shared policy sink.
//!
//! The layering is deliberate. Backends only decode; everything a file manager must get
//! right — entry-name decoding, path safety, size limits, progress, cancellation and the
//! atomic move into the destination — lives in [`sink`], so adding a format cannot skip
//! those rules.

pub mod format;

pub mod backends;
mod entry;
mod sink;
pub mod write;

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tauri::ipc::Channel;

use crate::error::ApiError;
use crate::ops::{OpProgress, OpState};

pub use sink::Sink;

/// Distinguishes concurrent extractions of the same inner path within this process.
static CACHE_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// One entry of an archive, as shown by the preview panel and virtual directory listing.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveEntry {
    pub name: String,
    pub size: u64,
    #[serde(default)]
    pub is_dir: bool,
    #[serde(default)]
    pub mtime: u64,
}

/// Entry names for the preview panel; capped by the caller.
pub fn preview_entries(path: &str, limit: usize) -> Result<Vec<ArchiveEntry>, ApiError> {
    backends::list(path, limit)
}

/// Resolves `path` to its underlying archive file and relative inner path, if it points to
/// an archive file or a path inside one.
pub fn resolve_archive_path(path: &str) -> Option<(std::path::PathBuf, String)> {
    let clean = path.trim_end_matches(['\\', '/']);
    let mut cur = Path::new(clean);
    if cur.is_dir() {
        return None;
    }
    if cur.is_file() {
        if format::detect(&cur.to_string_lossy()).is_some() {
            return Some((cur.to_path_buf(), String::new()));
        }
        return None;
    }
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    while let Some(parent) = cur.parent() {
        if let Some(file_name) = cur.file_name() {
            parts.push(file_name);
        }
        if parent.is_file() {
            if format::detect(&parent.to_string_lossy()).is_some() {
                parts.reverse();
                let inner = parts
                    .iter()
                    .map(|p| p.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("\\");
                return Some((parent.to_path_buf(), inner));
            }
            return None;
        }
        if parent.is_dir() {
            return None;
        }
        cur = parent;
    }
    None
}

/// True if `path` is an archive file or a valid directory inside an archive file.
pub fn is_archive_or_inner_dir(path: &str) -> bool {
    let Some((archive_path, inner_path)) = resolve_archive_path(path) else {
        return false;
    };
    if inner_path.is_empty() {
        return true;
    }
    is_inner_dir(&archive_path, &inner_path).unwrap_or(false)
}

/// Removes non-directory ancestors that would make `create_dir_all` fail. The archive may
/// have been replaced since the cache was written, so a path that used to be a file can now
/// be a parent folder (and vice versa).
fn clear_blocking_ancestors(root: &std::path::Path, path: &std::path::Path) {
    // `path` is the directory that must exist for the destination; it is often exactly the
    // entry an older archive revision cached as a file, so it must be inspected too.
    let mut current = Some(path);
    while let Some(dir) = current {
        if dir == root {
            break;
        }
        match std::fs::symlink_metadata(dir) {
            Ok(meta) if meta.is_dir() => break,
            Ok(_) => {
                let _ = std::fs::remove_file(dir);
            }
            Err(_) => {}
        }
        current = dir.parent();
    }
}

/// Checks whether `inner_path` represents a directory inside `archive_path`.
pub fn is_inner_dir(archive_path: &Path, inner_path: &str) -> Result<bool, ApiError> {
    let target = inner_path.replace('/', "\\");
    let target = target.trim_matches('\\').to_lowercase();
    if target.is_empty() {
        return Ok(true);
    }
    // Tar streams have no index. The dedicated scan stops at the first match, so opening
    // one member no longer decompresses the whole archive just to answer this question.
    // Other formats read an index and can use the complete listing below.
    if let Some(found) =
        backends::tar_contains_entry(&archive_path.to_string_lossy(), &target, 500_000)?
    {
        return Ok(found);
    }
    // Use the unbounded listing and refuse to guess when even that hits the entry cap: a
    // directory whose first member sits past a truncated listing used to be treated as a
    // file and materialized as a zero-byte entry.
    let (entries, truncated) =
        backends::list_unbounded_with_truncated(&archive_path.to_string_lossy(), 500_000)?;
    if truncated {
        return Err(ApiError::new(
            "TOO_MANY_ENTRIES",
            format!("more than {} entries", backends::MAX_MATERIALIZE_ENTRIES),
        ));
    }
    let prefix = format!("{target}\\");
    for entry in entries {
        let norm = entry.name.replace('/', "\\");
        let norm_lower = norm.trim_matches('\\').to_lowercase();
        if norm_lower.starts_with(&prefix) {
            return Ok(true);
        }
        if norm_lower == target && entry.is_dir {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Lists immediate children of `inner_path` inside `archive_path` as directory entries.
pub fn enumerate_virtual(
    archive_path: &Path,
    inner_path: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<crate::fs::ScanOutcome, ApiError> {
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};

    // Browsing must see every member. The preview variant stops a tar walk after 64 MiB
    // and would report a complete-looking folder missing everything past that point.
    let (entries, truncated) =
        backends::list_unbounded_with_truncated(&archive_path.to_string_lossy(), 500_000)?;
    let default_mtime = archive_path
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let target_inner = inner_path.replace('/', "\\");
    let target_inner = target_inner.trim_matches('\\').to_lowercase();

    let mut map: HashMap<String, crate::fs::Entry> = HashMap::new();
    let mut matched = false;

    let hash_key = |name: &str| -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        archive_path.hash(&mut hasher);
        target_inner.hash(&mut hasher);
        name.to_lowercase().hash(&mut hasher);
        hasher.finish()
    };

    for arc_entry in entries {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(ApiError::new("CANCELED", "enumeration canceled"));
        }
        let norm = arc_entry.name.replace('/', "\\");
        let norm = norm.trim_matches('\\');
        if norm.is_empty() {
            continue;
        }
        let rel_owned;
        let rel: &str = if target_inner.is_empty() {
            norm
        } else {
            // Compare segment by segment instead of slicing with a byte offset taken from
            // the lowercased string: lowercasing can change a character's UTF-8 length
            // (`İ` → `i̇`), and that offset would land inside the original string.
            let mut segments = norm.split('\\').filter(|segment| !segment.is_empty());
            let mut is_target = true;
            for target_segment in target_inner.split('\\') {
                match segments.next() {
                    Some(segment) if segment.to_lowercase() == target_segment => {}
                    _ => {
                        is_target = false;
                        break;
                    }
                }
            }
            if !is_target {
                continue;
            }
            matched = true;
            let rest: Vec<&str> = segments.collect();
            if rest.is_empty() {
                continue;
            }
            rel_owned = rest.join("\\");
            rel_owned.as_str()
        };
        let rel = rel.trim_matches('\\');
        if rel.is_empty() {
            continue;
        }
        let mtime = if arc_entry.mtime > 0 { arc_entry.mtime } else { default_mtime };
        if let Some((dir_name, _)) = rel.split_once('\\') {
            let key = dir_name.to_lowercase();
            map.entry(key).or_insert_with(|| {
                crate::fs::Entry::new(dir_name.to_string(), 0, mtime, 0x10, hash_key(dir_name))
            });
        } else {
            let key = rel.to_lowercase();
            let is_dir = arc_entry.is_dir;
            let attrs = if is_dir { 0x10 } else { 0 };
            let size = if is_dir { 0 } else { arc_entry.size };
            let id = hash_key(rel);
            map.entry(key)
                .and_modify(|existing| {
                    if !existing.is_dir() && is_dir {
                        *existing = crate::fs::Entry::new(rel.to_string(), 0, mtime, 0x10, id);
                    }
                })
                .or_insert_with(|| crate::fs::Entry::new(rel.to_string(), size, mtime, attrs, id));
        }
    }

    if !target_inner.is_empty() && !matched && map.is_empty() {
        return Err(ApiError::new("NOT_FOUND", inner_path));
    }

    Ok(crate::fs::ScanOutcome {
        entries: map.into_values().collect(),
        truncated,
    })
}

/// `inner_path` crosses IPC and is joined under the cache directory, so it must be a plain
/// relative path: no drive, UNC prefix, root or `..` component may escape the cache.
fn validate_inner_path(inner: &str) -> Result<(), ApiError> {
    if inner.is_empty() || inner.starts_with(['\\', '/']) || inner.contains(':') {
        return Err(ApiError::new("INVALID_PATH", inner));
    }
    if inner.split(['\\', '/']).any(|part| {
        part.is_empty()
            || part == "."
            || part == ".."
            // Win32 strips trailing dots/spaces from every component, so `.. ` resolves
            // to `..` and `name.` aliases `name`. Reject rather than resolve a path the
            // caller did not spell (this string crosses IPC and is joined under the cache).
            || part.ends_with([' ', '.'])
            || part.chars().any(char::is_control)
    }) {
        return Err(ApiError::new("INVALID_PATH", inner));
    }
    Ok(())
}

/// Root of the per-archive preview cache under `%TEMP%`.
fn cache_root() -> std::path::PathBuf {
    std::env::temp_dir().join("FlowDir").join("archive_cache")
}

/// Drops cache entries no run has touched for a week. Every previewed archive member is
/// extracted here and nothing else removes it, so without this the folder grows for the
/// life of the profile.
pub fn cleanup_cache() {
    const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);
    let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(cache_root()) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else { continue };
        let Ok(age) = modified.duration_since(std::time::UNIX_EPOCH) else {
            continue;
        };
        if now.saturating_sub(age) < MAX_AGE {
            continue;
        }
        let path = entry.path();
        let _ = if meta.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
    }
}

/// Extracts a single entry to the FlowDir temporary cache and returns the path on disk.
///
/// `limit` bounds the bytes written for one member; previews pass the configured preview
/// size so a hostile archive cannot decompress a multi-GB member into `%TEMP%`. User
/// initiated opens and drags pass `None`.
pub fn extract_cached(
    archive_path: &Path,
    inner_path: &str,
    limit: Option<u64>,
) -> Result<std::path::PathBuf, ApiError> {
    validate_inner_path(inner_path)?;
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    archive_path.hash(&mut hasher);
    let hash = hasher.finish();

    let temp = cache_root().join(format!("{hash:016x}"));
    let inner_norm = inner_path.replace('/', "\\");
    let dest = temp.join(inner_norm.trim_matches('\\'));

    let arc_mtime = archive_path
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let dest_mtime = dest
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    if dest.is_file() && dest_mtime >= arc_mtime && arc_mtime > 0 {
        return Ok(dest);
    }

    // A directory entry must be materialized as a tree: `extract_single` would write a
    // zero-byte file under the folder's name and every child would then fail.
    if is_inner_dir(archive_path, inner_path)? {
        if dest.is_dir() && dest_mtime >= arc_mtime && arc_mtime > 0 {
            return Ok(dest);
        }
        let parent = dest.parent().unwrap_or(temp.as_path());
        clear_blocking_ancestors(&temp, parent);
        std::fs::create_dir_all(parent)
            .map_err(|e| ApiError::from_io(&parent.to_string_lossy(), &e))?;
        // Build the tree next to the destination and rename it in: a member that fails
        // halfway must not leave a partial folder carrying a fresh mtime that the check
        // above would then serve as complete.
        let name = dest
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "entry".to_string());
        let tmp = dest.with_file_name(format!(
            ".{name}.{}.{}.tmp",
            std::process::id(),
            CACHE_TMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).map_err(|e| ApiError::from_io(&tmp.to_string_lossy(), &e))?;
        if let Err(err) =
            backends::extract_dir(
                &archive_path.to_string_lossy(),
                &inner_norm,
                &tmp,
                false,
                // Respect the caller's budget; a `None` (user-initiated open) still gets the
                // materialization ceiling so a hostile archive cannot fill `%TEMP%`.
                Some(limit.unwrap_or(backends::MAX_MATERIALIZE_BYTES)),
            )
        {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(err);
        }
        let _ = if dest.is_dir() {
            std::fs::remove_dir_all(&dest)
        } else {
            std::fs::remove_file(&dest)
        };
        std::fs::rename(&tmp, &dest).map_err(|e| {
            let _ = std::fs::remove_dir_all(&tmp);
            ApiError::from_io(&dest.to_string_lossy(), &e)
        })?;
        return Ok(dest);
    }

    if let Some(parent) = dest.parent() {
        clear_blocking_ancestors(&temp, parent);
        std::fs::create_dir_all(parent).map_err(|e| ApiError::from_io(&parent.to_string_lossy(), &e))?;
    }

    // Decode into a sibling temp file: a member that fails mid-write must not leave a
    // truncated file behind that the mtime check above would keep serving.
    let name = dest
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "entry".to_string());
    let tmp = dest.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        CACHE_TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&tmp);
    if let Err(err) = backends::extract_single(&archive_path.to_string_lossy(), &inner_norm, &tmp, None, limit) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    // The cache may still hold a directory at this path from an older archive revision;
    // renaming a file over it fails until the entry is removed.
    if dest.is_dir() {
        let _ = std::fs::remove_dir_all(&dest);
    }
    std::fs::rename(&tmp, &dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        ApiError::from_io(&dest.to_string_lossy(), &e)
    })?;
    Ok(dest)
}

/// Starts extraction on a background thread. Preconditions are checked before spawning so
/// the command can report them as a normal error.
pub fn start(
    state: &OpState,
    job_id: String,
    archive: String,
    dest: String,
    password: Option<String>,
    channel: Channel<OpProgress>,
) -> Result<(), ApiError> {
    if !Path::new(&archive).is_file() {
        return Err(ApiError::new("NOT_FOUND", archive));
    }
    if format::detect(&archive).is_none() {
        return Err(ApiError::new("UNSUPPORTED_FORMAT", archive));
    }
    // Registered in the shared op state so `op_cancel` and the quit guard already know
    // about extraction jobs.
    let token = state.begin_external(&job_id);
    let finish_token = token.clone();
    let err_token = finish_token.clone();
    let err_job_id = job_id.clone();
    let err_state = state.clone();
    let state = state.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("flowdir-extract-{job_id}"))
        .spawn(move || {
            run(job_id.clone(), archive, dest, password, token, channel);
            state.finish_external(&job_id, &finish_token);
        });
    // Spawn failure would otherwise leave a ghost job that blocks `wait_idle` forever.
    if spawned.is_err() {
        err_state.finish_external(&err_job_id, &err_token);
    }
    spawned.map(|_| ()).map_err(|e| ApiError::new("THREAD", e.to_string()))
}

/// Runs one extraction to completion, emitting progress and a final message.
fn run(
    job_id: String,
    archive: String,
    dest: String,
    password: Option<String>,
    cancel: Arc<AtomicBool>,
    channel: Channel<OpProgress>,
) -> OpProgress {
    log::info!(target: "flowdir::archive", "extract {archive} -> {dest}");
    let mut sink = match Sink::new(job_id.clone(), Path::new(&dest), cancel, channel.clone()) {
        Ok(sink) => sink,
        Err(err) => {
            log::error!(target: "flowdir::archive", "extract {archive} failed: {} {}", err.code, err.detail);
            let progress = OpProgress {
                op_id: job_id,
                phase: "done".into(),
                current: 0,
                total: 0,
                name: None,
                work: 0,
                work_total: 0,
                failed_count: 0,
                failed: Vec::new(),
                succeeded: Vec::new(),
                failed_indices: Vec::new(),
                landed: Vec::new(),
                done: true,
                aborted: false,
                error: Some(err),
            };
            let _ = channel.send(progress.clone());
            return progress;
        }
    };

    let result =
        backends::extract(&archive, &mut sink, password.as_deref()).and_then(|_| sink.merge());
    let aborted = matches!(&result, Err(err) if err.code == "CANCELED");
    // Decoding into the temporary tree is all-or-nothing: a cancel before the merge can
    // drop everything. Once merge has started, entries already moved into the destination
    // stay there, so the user must not be told the result is a clean "canceled".
    let merge_partial = aborted && sink.merge_started();
    let error = match result {
        Ok(()) if sink.failed_total() > 0 => {
            log::warn!(
                target: "flowdir::archive",
                "extract {archive} partial: {} entries skipped or failed",
                sink.failed_total()
            );
            None
        }
        Ok(()) => {
            log::info!(
                target: "flowdir::archive",
                "extract {archive} done: {} entries, {} bytes",
                sink.processed(),
                sink.bytes()
            );
            None
        }
        Err(_err) if aborted && merge_partial => {
            log::warn!(
                target: "flowdir::archive",
                "extract {archive} canceled during merge; {dest} may be incomplete"
            );
            Some(ApiError::new("CANCELED_PARTIAL", dest))
        }
        Err(_err) if aborted => {
            log::warn!(target: "flowdir::archive", "extract {archive} canceled");
            None
        }
        Err(err) => {
            log::error!(target: "flowdir::archive", "extract {archive} failed: {} {}", err.code, err.detail);
            Some(err)
        }
    };

    let progress = OpProgress {
        op_id: job_id,
        phase: "done".into(),
        current: sink.processed(),
        total: sink.total(),
        name: None,
        work: sink.bytes(),
        work_total: sink.bytes(),
        failed_count: sink.failed_total(),
        failed: sink.failures().to_vec(),
        succeeded: Vec::new(),
        failed_indices: Vec::new(),
        landed: Vec::new(),
        done: true,
        aborted,
        error,
    };
    let _ = channel.send(progress.clone());
    progress
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flowdir-archive-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn extract(archive: &Path, dest: &Path) -> OpProgress {
        extract_with(archive, dest, None)
    }

    fn extract_with(archive: &Path, dest: &Path, password: Option<&str>) -> OpProgress {
        run(
            "test-job".into(),
            archive.to_string_lossy().into_owned(),
            dest.to_string_lossy().into_owned(),
            password.map(str::to_string),
            Arc::new(AtomicBool::new(false)),
            Channel::new(|_| Ok(())),
        )
    }

    #[test]
    fn inner_paths_cannot_use_win32_aliases() {
        // Win32 trims trailing dots/spaces per component, so these would resolve outside
        // the cache root (`.. ` becomes `..`).
        for inner in [".. ", "a/.. ", "a./b", "a /b", "a/...", "a/\u{1}b"] {
            assert!(validate_inner_path(inner).is_err(), "{inner:?}");
        }
        for inner in ["a/b.txt", "a/.gitignore", "dir name/file"] {
            assert!(validate_inner_path(inner).is_ok(), "{inner:?}");
        }
    }

    #[test]
    fn clear_blocking_ancestors_inspects_the_parent_itself() {
        let dir = temp_dir("blocking-parent");
        let root = dir.join("cache");
        std::fs::create_dir_all(&root).unwrap();
        // An older archive revision cached `docs` as a file; the destination now needs it
        // to be a directory.
        let parent = root.join("docs");
        std::fs::write(&parent, b"stale").unwrap();
        clear_blocking_ancestors(&root, &parent);
        assert!(!parent.exists());
        std::fs::create_dir_all(&parent).unwrap();
        assert!(parent.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_member_is_skipped_without_aborting() {
        let dir = temp_dir("corrupt-member");
        let archive = dir.join("src.zip");
        let file = std::fs::File::create(&archive).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("good.txt", stored).unwrap();
        zip.write_all(b"good").unwrap();
        zip.start_file("bad.txt", stored).unwrap();
        zip.write_all(b"alpha payload").unwrap();
        zip.finish().unwrap();
        // Stored members keep their payload verbatim; one flipped byte breaks that
        // member's CRC while the rest of the archive stays readable.
        let mut bytes = std::fs::read(&archive).unwrap();
        let pos = bytes
            .windows(7)
            .position(|window| window == b"alpha p")
            .expect("stored payload");
        bytes[pos] ^= 0xFF;
        std::fs::write(&archive, &bytes).unwrap();

        let dest = dir.join("out");
        let progress = extract(&archive, &dest);
        assert!(progress.error.is_none(), "{:?}", progress.error);
        assert_eq!(std::fs::read(dest.join("good.txt")).unwrap(), b"good");
        assert!(!dest.join("bad.txt").exists());
        assert_eq!(progress.failed_count, 1, "{:?}", progress.failed);
        assert_eq!(progress.failed[0].code, "CORRUPT");
    }

    #[test]
    fn zip_names_dirs_and_unsafe_entries() {
        let dir = temp_dir("zip");
        let archive = dir.join("sample.zip");
        let file = std::fs::File::create(&archive).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, body) in [
            ("相册/照片.txt", "中文内容".as_bytes()),
            ("plain.txt", b"plain" as &[u8]),
            ("../escape.txt", b"nope"),
            ("CON.txt", b"device"),
        ] {
            writer.start_file(name, options).unwrap();
            writer.write_all(body).unwrap();
        }
        writer.finish().unwrap();

        let dest = dir.join("out");
        let progress = extract(&archive, &dest);
        assert!(progress.error.is_none(), "{:?}", progress.error);

        assert_eq!(
            std::fs::read_to_string(dest.join("相册\\照片.txt")).unwrap(),
            "中文内容"
        );
        assert!(dest.join("plain.txt").is_file());
        assert!(dest.join("_CON.txt").is_file());
        assert!(!dir.join("escape.txt").exists());
        // Three entries accepted, one refused and reported; the counts the UI derives
        // from these fields must stay consistent.
        assert_eq!(progress.failed_count, 1);
        assert_eq!(progress.current, 4);
        assert_eq!(progress.total, 4);
    }

    #[test]
    fn listing_a_folder_whose_lowercase_grows() {
        let dir = temp_dir("lowercase");
        let archive = dir.join("i.zip");
        let file = std::fs::File::create(&archive).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for name in ["İ/İ.txt", "İ/other.txt"] {
            writer.start_file(name, options).unwrap();
            writer.write_all(b"x").unwrap();
        }
        writer.finish().unwrap();

        // `İ`.to_lowercase() is three bytes long, so a byte offset taken from the
        // lowercased name used to slice inside the original name and panic.
        let listing = enumerate_virtual(&archive, "İ", &Arc::new(AtomicBool::new(false))).unwrap();
        let names: Vec<&str> = listing.entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"İ.txt"));
        assert!(names.contains(&"other.txt"));
    }

    #[test]
    fn preview_lists_names_without_unsafe_entries() {
        let dir = temp_dir("preview");
        let archive = dir.join("sample.zip");
        let file = std::fs::File::create(&archive).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for name in ["相册/照片.txt", "plain.txt", "../escape.txt"] {
            writer.start_file(name, options).unwrap();
            writer.write_all(b"x").unwrap();
        }
        writer.finish().unwrap();

        let entries = preview_entries(archive.to_str().unwrap(), 10).unwrap();
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["相册\\照片.txt", "plain.txt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sevenz_backslash_names_extract_nested() {
        let dir = temp_dir("sevenz-backslash");
        let seven = dir.join("win.7z");
        let mut writer = sevenz_rust2::ArchiveWriter::create(&seven).unwrap();
        writer
            .push_archive_entry(
                sevenz_rust2::ArchiveEntry::new_file("相册\\照片.txt"),
                Some(&b"win body"[..]),
            )
            .unwrap();
        writer.finish().unwrap();

        let dest = dir.join("out");
        let progress = extract(&seven, &dest);
        assert!(progress.error.is_none(), "{:?}", progress.error);
        assert_eq!(progress.failed_count, 0);
        assert_eq!(std::fs::read(dest.join("相册").join("照片.txt")).unwrap(), b"win body");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sevenz_and_cab_round_trip() {
        let dir = temp_dir("sevenz-cab");

        let seven = dir.join("sample.7z");
        let mut writer = sevenz_rust2::ArchiveWriter::create(&seven).unwrap();
        writer
            .push_archive_entry(
                sevenz_rust2::ArchiveEntry::new_file("相册/照片.txt"),
                Some(&b"7z body"[..]),
            )
            .unwrap();
        writer
            .push_archive_entry(
                sevenz_rust2::ArchiveEntry::new_file("plain.txt"),
                Some(&b"plain"[..]),
            )
            .unwrap();
        writer.finish().unwrap();

        let dest = dir.join("out7z");
        let progress = extract(&seven, &dest);
        assert!(progress.error.is_none(), "{:?}", progress.error);
        assert_eq!(std::fs::read(dest.join("相册\\照片.txt")).unwrap(), b"7z body");
        assert_eq!(std::fs::read(dest.join("plain.txt")).unwrap(), b"plain");

        let cab = dir.join("sample.cab");
        let file = std::fs::File::create(&cab).unwrap();
        let mut builder = cab::CabinetBuilder::new();
        builder
            .add_folder(cab::CompressionType::MsZip)
            .add_file("docs/说明.txt");
        let mut writer = builder.build(file).unwrap();
        // Payloads are written in the same order the files were declared.
        writer
            .next_file()
            .unwrap()
            .unwrap()
            .write_all(b"cab body")
            .unwrap();
        writer.finish().unwrap();

        let dest = dir.join("outcab");
        let progress = extract(&cab, &dest);
        assert!(progress.error.is_none(), "{:?}", progress.error);
        assert_eq!(std::fs::read(dest.join("docs\\说明.txt")).unwrap(), b"cab body");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rar_volumes_are_joined() {
        let solo_dir = temp_dir("rar5vol-solo");
        let solo = solo_dir.join("rar5_multi.part1.rar");
        std::fs::write(&solo, include_bytes!("../../tests/data/rar5_multi.part1.rar")).unwrap();
        let alone = extract(&solo, &solo_dir.join("out"));
        assert!(
            alone.error.is_some(),
            "one volume cannot decode members that continue in the others"
        );

        let dir = temp_dir("rar5vol");
        for (part, bytes) in [
            (1, &include_bytes!("../../tests/data/rar5_multi.part1.rar")[..]),
            (2, &include_bytes!("../../tests/data/rar5_multi.part2.rar")[..]),
            (3, &include_bytes!("../../tests/data/rar5_multi.part3.rar")[..]),
            (4, &include_bytes!("../../tests/data/rar5_multi.part4.rar")[..]),
        ] {
            std::fs::write(dir.join(format!("rar5_multi.part{part}.rar")), bytes).unwrap();
        }
        let first = dir.join("rar5_multi.part1.rar");
        let dest = dir.join("out");
        let progress = extract(&first, &dest);
        assert!(progress.error.is_none(), "{:?}", progress.error);
        assert_eq!(progress.failed_count, 0);
        assert_eq!(std::fs::read(dest.join("cebula.txt")).unwrap().len(), 814);
        assert_eq!(std::fs::read(dest.join("elf-Linux-ARMv7-ls")).unwrap().len(), 90808);
        assert_eq!(preview_entries(first.to_str().unwrap(), 100).unwrap().len(), 9);

        // Starting from a middle volume still resolves the set from its siblings.
        let late = dir.join("late");
        let progress = extract(&dir.join("rar5_multi.part3.rar"), &late);
        assert!(progress.error.is_none(), "{:?}", progress.error);
        assert_eq!(std::fs::read(late.join("test6.bin")).unwrap().len(), 4096);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rar_extracts_and_asks_for_a_password() {
        let dir = temp_dir("rar");
        let plain = dir.join("plain.rar");
        std::fs::write(&plain, include_bytes!("../../tests/data/rar4.rar")).unwrap();
        let dest = dir.join("out");
        let progress = extract(&plain, &dest);
        assert!(progress.error.is_none(), "{:?}", progress.error);
        assert_eq!(std::fs::read(dest.join("test.txt")).unwrap().len(), 20);
        assert_eq!(std::fs::read(dest.join("testdir\\test.txt")).unwrap().len(), 20);
        assert!(dest.join("testemptydir").is_dir());
        // `testlink` is a symlink member and must be reported, not materialised.
        assert_eq!(progress.failed_count, 1);

        let encrypted = dir.join("enc.rar");
        std::fs::write(&encrypted, include_bytes!("../../tests/data/rar_enc.rar")).unwrap();
        let locked = dir.join("locked");
        let progress = extract(&encrypted, &locked);
        let error = progress.error.expect("password must be requested");
        assert_eq!(error.code, "PASSWORD_REQUIRED");
        let left = std::fs::read_dir(&locked).map(|mut it| it.next().is_none()).unwrap_or(true);
        assert!(left, "nothing may be written before the password is known");

        let dest = dir.join("unlocked");
        let progress = extract_with(&encrypted, &dest, Some("12345678"));
        assert!(progress.error.is_none(), "{:?}", progress.error);
        assert_eq!(std::fs::read(dest.join("foo.txt")).unwrap().len(), 16);
        assert_eq!(std::fs::read(dest.join("bar.txt")).unwrap().len(), 16);

        let wrong = extract_with(&encrypted, dir.join("wrong").as_path(), Some("bad"));
        let wrong_error = wrong.error.expect("wrong password");
        assert_eq!(wrong_error.code, "WRONG_PASSWORD", "{wrong_error:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn encrypted_zip_needs_the_right_password() {
        let dir = temp_dir("enc-zip");
        let archive = dir.join("secret.zip");
        let file = std::fs::File::create(&archive).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .with_aes_encryption(zip::AesMode::Aes256, "hunter2");
        writer.start_file("secret.txt", options).unwrap();
        writer.write_all(b"top secret").unwrap();
        writer.finish().unwrap();

        let locked = dir.join("locked");
        assert_eq!(
            extract(&archive, &locked).error.expect("password").code,
            "PASSWORD_REQUIRED"
        );
        let dest = dir.join("out");
        let progress = extract_with(&archive, &dest, Some("hunter2"));
        assert!(progress.error.is_none(), "{:?}", progress.error);
        assert_eq!(std::fs::read(dest.join("secret.txt")).unwrap(), b"top secret");
        assert_eq!(
            extract_with(&archive, dir.join("bad").as_path(), Some("nope"))
                .error
                .expect("wrong password")
                .code,
            "WRONG_PASSWORD"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_extracts_on_a_background_thread() {
        let dir = temp_dir("start");
        let archive = dir.join("job.zip");
        let file = std::fs::File::create(&archive).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("only.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"started").unwrap();
        writer.finish().unwrap();

        let dest = dir.join("out");
        let state = OpState::default();
        start(
            &state,
            "start-job".into(),
            archive.to_string_lossy().into_owned(),
            dest.to_string_lossy().into_owned(),
            None,
            Channel::new(|_| Ok(())),
        )
        .unwrap();
        let target = dest.join("only.txt");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::fs::read(&target).ok().as_deref() != Some(b"started") {
            assert!(std::time::Instant::now() < deadline, "extraction did not finish");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // The job must leave the shared op state, or quitting would keep waiting for it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !state.active().is_empty() {
            assert!(std::time::Instant::now() < deadline, "job stayed registered");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tar_gz_round_trip_and_collisions() {
        let dir = temp_dir("targz");
        let archive = dir.join("bundle.tar.gz");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
            let mut builder = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            let body = b"tar body";
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "docs/说明.txt", &body[..])
                .unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }

        let dest = dir.join("out");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("说明.txt"), b"existing").unwrap();

        let progress = extract(&archive, &dest);
        assert!(progress.error.is_none(), "{:?}", progress.error);
        assert_eq!(std::fs::read(dest.join("docs\\说明.txt")).unwrap(), b"tar body");
        // A collision at the destination is renamed, never overwritten.
        assert_eq!(std::fs::read(dest.join("说明.txt")).unwrap(), b"existing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn virtual_archive_directory_navigation_and_extraction() {
        let dir = temp_dir("virtual_dir");
        let archive = dir.join("test.zip");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("folder/nested/item.txt", opts).unwrap();
            std::io::Write::write_all(&mut zip, b"nested content").unwrap();
            zip.start_file("root.txt", opts).unwrap();
            std::io::Write::write_all(&mut zip, b"root content").unwrap();
            zip.finish().unwrap();
        }

        // 1. resolve_archive_path
        let res_root = resolve_archive_path(archive.to_str().unwrap()).unwrap();
        assert_eq!(res_root.0, archive);
        assert_eq!(res_root.1, "");

        let nested_path = format!("{}\\folder\\nested\\item.txt", archive.display());
        let res_nested = resolve_archive_path(&nested_path).unwrap();
        assert_eq!(res_nested.0, archive);
        assert_eq!(res_nested.1, "folder\\nested\\item.txt");

        let non_archive = dir.join("not_archive.txt");
        std::fs::write(&non_archive, b"hi").unwrap();
        assert!(resolve_archive_path(non_archive.to_str().unwrap()).is_none());
        assert!(resolve_archive_path(dir.to_str().unwrap()).is_none());

        // 2. enumerate_virtual root
        let cancel = Arc::new(AtomicBool::new(false));
        let root_listing = enumerate_virtual(&archive, "", &cancel).unwrap();
        assert_eq!(root_listing.entries.len(), 2);
        let folder_entry = root_listing.entries.iter().find(|e| e.name == "folder").unwrap();
        assert!(folder_entry.is_dir());
        let root_file = root_listing.entries.iter().find(|e| e.name == "root.txt").unwrap();
        assert!(!root_file.is_dir());
        assert_eq!(root_file.size, 12);

        // 3. enumerate_virtual subfolder
        let sub_listing = enumerate_virtual(&archive, "folder", &cancel).unwrap();
        assert_eq!(sub_listing.entries.len(), 1);
        assert_eq!(sub_listing.entries[0].name, "nested");
        assert!(sub_listing.entries[0].is_dir());

        let deep_listing = enumerate_virtual(&archive, "folder\\nested", &cancel).unwrap();
        assert_eq!(deep_listing.entries.len(), 1);
        assert_eq!(deep_listing.entries[0].name, "item.txt");
        assert!(!deep_listing.entries[0].is_dir());
        assert_eq!(deep_listing.entries[0].size, 14);

        // 4. is_archive_or_inner_dir & is_inner_dir
        assert!(is_archive_or_inner_dir(archive.to_str().unwrap()));
        assert!(is_archive_or_inner_dir(&format!("{}\\folder", archive.display())));
        assert!(is_archive_or_inner_dir(&format!("{}\\folder\\nested", archive.display())));
        assert!(!is_archive_or_inner_dir(&format!("{}\\nonexistent", archive.display())));
        assert!(is_inner_dir(&archive, "folder").unwrap());
        assert!(!is_inner_dir(&archive, "folder\\nested\\item.txt").unwrap());

        // 5. extract_cached
        let cached = extract_cached(&archive, "folder\\nested\\item.txt", None).unwrap();
        assert!(cached.is_file());
        assert_eq!(std::fs::read(&cached).unwrap(), b"nested content");

        // 6. 7z and tar.gz single extraction
        let sevenz_path = dir.join("test.7z");
        {
            let mut writer = sevenz_rust2::ArchiveWriter::create(&sevenz_path).unwrap();
            writer
                .push_archive_entry(
                    sevenz_rust2::ArchiveEntry::new_file("deep/sub/doc.txt"),
                    Some(&b"7z deep content"[..]),
                )
                .unwrap();
            writer.finish().unwrap();
        }
        let cached_7z = extract_cached(&sevenz_path, "deep\\sub\\doc.txt", None).unwrap();
        assert!(cached_7z.is_file());
        assert_eq!(std::fs::read(&cached_7z).unwrap(), b"7z deep content");

        let targz_path = dir.join("test.tar.gz");
        {
            let file = std::fs::File::create(&targz_path).unwrap();
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
            let mut builder = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            let body = b"tar content";
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, "a/b/file.txt", &body[..]).unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let cached_tar = extract_cached(&targz_path, "a\\b\\file.txt", None).unwrap();
        assert!(cached_tar.is_file());
        assert_eq!(std::fs::read(&cached_tar).unwrap(), b"tar content");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `inner_path` comes over IPC and is joined under `%TEMP%`; a traversal must not be
    /// able to create or overwrite anything outside the archive cache.
    #[test]
    fn cached_extraction_rejects_escaping_inner_paths() {
        let dir = temp_dir("inner-path");
        let archive = dir.join("a.zip");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file("a.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"body").unwrap();
            writer.finish().unwrap();
        }
        for bad in [r"..\evil.txt", r"a\..\..\evil.txt", r"\absolute.txt", r"D:\evil.txt", ""] {
            let err = extract_cached(&archive, bad, None).unwrap_err();
            assert_eq!(err.code, "INVALID_PATH", "{bad}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A destination that already holds a file with the archive folder's name used to
    /// abort the whole merge (and delete the files already moved). The folder is renamed
    /// instead, and its children follow the renamed folder.
    #[test]
    fn merge_relocates_a_colliding_folder() {
        let dir = temp_dir("merge-collision");
        let archive = dir.join("tree.zip");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            writer.add_directory("docs/", options).unwrap();
            writer.start_file("docs/a.txt", options).unwrap();
            writer.write_all(b"child").unwrap();
            writer.finish().unwrap();
        }
        let dest = dir.join("out");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("docs"), b"existing file").unwrap();

        let progress = extract(&archive, &dest);
        assert!(progress.error.is_none(), "{:?}", progress.error);
        assert_eq!(std::fs::read(dest.join("docs")).unwrap(), b"existing file");
        assert_eq!(
            std::fs::read(dest.join("docs (1)").join("a.txt")).unwrap(),
            b"child"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A preview must not decompress a member past its budget: the declared size is
    /// attacker-controlled, so the cap has to hold on the byte stream itself.
    #[test]
    fn cached_extraction_honours_the_byte_budget() {
        let dir = temp_dir("extract-budget");
        let archive = dir.join("bomb.zip");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("big.txt", options).unwrap();
            writer.write_all(&vec![b'x'; 200_000]).unwrap();
            writer.start_file("small.txt", options).unwrap();
            writer.write_all(b"tiny").unwrap();
            writer.finish().unwrap();
        }

        let err = extract_cached(&archive, "big.txt", Some(1024)).unwrap_err();
        assert_eq!(err.code, "TOO_LARGE");

        let cached = extract_cached(&archive, "small.txt", Some(1024)).unwrap();
        assert_eq!(std::fs::read(&cached).unwrap(), b"tiny");

        // Without a preview budget the same member extracts in full.
        let full = extract_cached(&archive, "big.txt", None).unwrap();
        assert_eq!(std::fs::metadata(&full).unwrap().len(), 200_000);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
