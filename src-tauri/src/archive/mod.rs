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

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::Serialize;
use tauri::ipc::Channel;

use crate::error::ApiError;
use crate::ops::{OpProgress, OpState};

pub use sink::Sink;

/// One entry of an archive, as shown by the preview panel.
#[derive(Serialize, Clone)]
pub struct ArchiveEntry {
    pub name: String,
    pub size: u64,
}

/// Entry names for the preview panel; capped by the caller.
pub fn preview_entries(path: &str, limit: usize) -> Result<Vec<ArchiveEntry>, ApiError> {
    backends::list(path, limit)
}

/// Starts extraction on a background thread. Preconditions are checked before spawning so
/// the command can report them as a normal error.
pub fn start(
    state: &OpState,
    job_id: String,
    archive: String,
    dest: String,
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
            run(job_id.clone(), archive, dest, token, channel);
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

    let result = backends::extract(&archive, &mut sink).and_then(|_| sink.merge());
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
        run(
            "test-job".into(),
            archive.to_string_lossy().into_owned(),
            dest.to_string_lossy().into_owned(),
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
}
