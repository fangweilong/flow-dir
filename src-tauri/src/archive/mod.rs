//! Archive extraction: format detection, per-format backends and the shared policy sink.
//!
//! The layering is deliberate. Backends only decode; everything a file manager must get
//! right — entry-name decoding, path safety, size limits, progress, cancellation and the
//! atomic move into the destination — lives in [`sink`], so adding a format cannot skip
//! those rules.

pub mod format;

mod backends;
mod entry;
mod sink;
pub mod write;

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::Serialize;
use tauri::ipc::Channel;

use crate::error::ApiError;
use crate::ops::{OpProgress, OpState};

pub use sink::Sink;

/// One entry of an archive, as shown by the preview panel and virtual directory listing.
#[derive(Serialize, Clone)]
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

/// Checks whether `inner_path` represents a directory inside `archive_path`.
pub fn is_inner_dir(archive_path: &Path, inner_path: &str) -> Result<bool, ApiError> {
    let entries = backends::list(&archive_path.to_string_lossy(), 500_000)?;
    let target = inner_path.replace('/', "\\");
    let target = target.trim_matches('\\').to_lowercase();
    if target.is_empty() {
        return Ok(true);
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

    let entries = backends::list(&archive_path.to_string_lossy(), 500_000)?;
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
        let norm_lower = norm.to_lowercase();
        let rel = if target_inner.is_empty() {
            norm
        } else {
            let prefix = format!("{target_inner}\\");
            if norm_lower.starts_with(&prefix) {
                matched = true;
                &norm[prefix.len()..]
            } else if norm_lower == target_inner {
                matched = true;
                continue;
            } else {
                continue;
            }
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
        truncated: false,
    })
}

/// Extracts a single entry to the FlowDir temporary cache and returns the path on disk.
pub fn extract_cached(archive_path: &Path, inner_path: &str) -> Result<std::path::PathBuf, ApiError> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    archive_path.hash(&mut hasher);
    let hash = hasher.finish();

    let temp = std::env::temp_dir().join("FlowDir").join("archive_cache").join(format!("{hash:016x}"));
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

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ApiError::from_io(&parent.to_string_lossy(), &e))?;
    }

    backends::extract_single(&archive_path.to_string_lossy(), &inner_norm, &dest, None)?;
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
    let state = state.clone();
    std::thread::Builder::new()
        .name(format!("flowdir-extract-{job_id}"))
        .spawn(move || {
            run(job_id.clone(), archive, dest, password, token, channel);
            state.finish_external(&job_id);
        })
        .map(|_| ())
        .map_err(|e| ApiError::new("THREAD", e.to_string()))
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
        let cached = extract_cached(&archive, "folder\\nested\\item.txt").unwrap();
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
        let cached_7z = extract_cached(&sevenz_path, "deep\\sub\\doc.txt").unwrap();
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
        let cached_tar = extract_cached(&targz_path, "a\\b\\file.txt").unwrap();
        assert!(cached_tar.is_file());
        assert_eq!(std::fs::read(&cached_tar).unwrap(), b"tar content");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
