//! The only place where extracted bytes reach the disk.
//!
//! Backends decode and hand over a reader; this sink decides the file name, enforces the
//! size/entry limits, counts progress, honours cancellation and moves the temporary tree
//! into the destination once the archive is done. Keeping that policy here means a new
//! backend cannot forget it.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::ipc::Channel;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

use crate::error::ApiError;
use crate::naming;
use crate::ops::{OpFailure, OpProgress};

/// Read size per write; large enough to keep syscall overhead low, small enough that
/// cancel and progress stay responsive.
const CHUNK: usize = 256 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(80);
const MAX_ENTRIES: u32 = 200_000;
const MAX_REPORTED_FAILURES: usize = 200;

/// What the sink decided to do with one entry.
pub enum Take {
    Dir,
    File(PathBuf),
    Skip,
}

struct Progress {
    job_id: String,
    channel: Channel<OpProgress>,
    phase: &'static str,
    entries: u32,
    entries_total: u32,
    bytes: u64,
    bytes_total: u64,
    failed: u32,
    last: Instant,
}

impl Progress {
    fn send(&mut self, name: Option<String>) {
        let _ = self.channel.send(OpProgress {
            op_id: self.job_id.clone(),
            phase: self.phase.to_string(),
            current: self.entries,
            total: self.entries_total,
            name,
            work: self.bytes,
            work_total: self.bytes_total,
            failed_count: self.failed,
            failed: Vec::new(),
            done: false,
            aborted: false,
            error: None,
        });
    }

    fn tick(&mut self, name: Option<String>) {
        if self.last.elapsed() >= PROGRESS_INTERVAL {
            self.last = Instant::now();
            self.send(name);
        }
    }
}

pub struct Sink {
    temp: PathBuf,
    dest: PathBuf,
    cancel: Arc<AtomicBool>,
    progress: Progress,
    used: HashSet<String>,
    current: Option<String>,
    /// Bytes still allowed to be written; the disk's free space at start, minus a margin.
    budget: u64,
    failures: Vec<OpFailure>,
    failed_total: u32,
    /// Entries the sink saw and refused (unsafe name, link, encrypted member).
    skipped: u32,
    merged: bool,
    buf: Vec<u8>,
}

impl Sink {
    pub fn new(
        job_id: String,
        dest: &Path,
        cancel: Arc<AtomicBool>,
        channel: Channel<OpProgress>,
    ) -> Result<Self, ApiError> {
        let dest_text = dest.to_string_lossy().into_owned();
        fs::create_dir_all(dest).map_err(|e| ApiError::from_io(&dest_text, &e))?;
        let dest = fs::canonicalize(dest).map_err(|e| ApiError::from_io(&dest_text, &e))?;
        let dest = verbatim(&dest);
        let temp = dest.join(format!(".flowdir-extract-{job_id}"));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir(&temp).map_err(|e| ApiError::from_io(temp.to_string_lossy().as_ref(), &e))?;
        // A volume that refuses the query (some network shares do) must not block
        // extraction; a real full disk still fails on the write itself.
        let free = free_space(&dest);
        let budget = if free == 0 {
            u64::MAX
        } else {
            free.saturating_sub(16 * 1024 * 1024)
        };
        Ok(Self {
            temp,
            dest,
            cancel,
            progress: Progress {
                job_id,
                channel,
                phase: "extract",
                entries: 0,
                entries_total: 0,
                bytes: 0,
                bytes_total: 0,
                failed: 0,
                last: Instant::now(),
            },
            used: HashSet::new(),
            current: None,
            budget,
            failures: Vec::new(),
            failed_total: 0,
            skipped: 0,
            merged: false,
            buf: vec![0u8; CHUNK],
        })
    }

    /// Declared totals, when the format knows them up front (zip's central directory).
    /// Switches the phase label the UI shows, e.g. from `plan` to `extract`.
    pub fn progress_phase(&mut self, phase: &'static str) {
        self.progress.phase = phase;
        self.progress.send(self.current.clone());
    }

    pub fn set_totals(&mut self, entries: Option<u32>, bytes: Option<u64>) -> Result<(), ApiError> {
        if entries == Some(0) {
            return Err(ApiError::new("EMPTY_ARCHIVE", "archive holds no entries"));
        }
        if let Some(bytes) = bytes {
            if bytes > self.budget {
                return Err(ApiError::new(
                    "NOT_ENOUGH_SPACE",
                    format!("archive needs {bytes} bytes, {} available", self.budget),
                ));
            }
        }
        self.progress.entries_total = entries.unwrap_or(0);
        self.progress.bytes_total = bytes.unwrap_or(0);
        Ok(())
    }

    /// Accepts one entry: enforces the entry cap, resolves duplicate names and prepares
    /// the placeholder. Returns [`Take::Skip`] for links and unsupported entries.
    pub fn accept(&mut self, rel: &str, is_dir: bool, link: bool) -> Result<Take, ApiError> {
        self.progress.entries += 1;
        if self.progress.entries > MAX_ENTRIES {
            return Err(ApiError::new(
                "TOO_MANY_ENTRIES",
                format!("archive holds more than {MAX_ENTRIES} entries"),
            ));
        }
        if link {
            self.skip(rel, "SYMLINK");
            return Ok(Take::Skip);
        }
        let rel = self.unique_rel(rel);
        let full = self.temp.join(&rel);
        self.current = Some(rel.clone());
        if is_dir {
            fs::create_dir_all(&full).map_err(|e| self.abort_io(Path::new(&rel), &e))?;
            self.progress.tick(Some(rel));
            Ok(Take::Dir)
        } else {
            Ok(Take::File(full))
        }
    }

    /// Streams one file entry to the temporary tree.
    pub fn write(&mut self, path: &Path, reader: &mut dyn Read, declared: u64) -> Result<(), ApiError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| self.abort_io(path, &e))?;
        }
        let mut file = match File::create(path) {
            Ok(file) => file,
            Err(err) => {
                self.skip_current(&ApiError::from_io(&display(path), &err));
                return Ok(());
            }
        };
        let mut buf = std::mem::take(&mut self.buf);
        let mut written = 0u64;
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                drop(file);
                let _ = fs::remove_file(path);
                self.buf = buf;
                return Err(ApiError::new("CANCELED", "extraction canceled"));
            }
            let read = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(read) => read,
                Err(err) => {
                    drop(file);
                    let _ = fs::remove_file(path);
                    self.buf = buf;
                    // A truncated or corrupt member should not take the whole archive
                    // down; the outer file manager reports it like any other failure.
                    self.skip_current(&ApiError::new("CORRUPT", format!("{err}")));
                    return Ok(());
                }
            };
            let written_total = written + read as u64;
            // A member that already exceeds its declared size is a lying header, and a
            // job that exceeds the free space would fail mid-write anyway.
            if declared > 0 && written_total > declared {
                drop(file);
                let _ = fs::remove_file(path);
                self.buf = buf;
                return Err(ApiError::new(
                    "TOO_LARGE",
                    format!("{} exceeds its declared size", self.current.as_deref().unwrap_or("entry")),
                ));
            }
            if self.progress.bytes + written_total > self.budget {
                drop(file);
                let _ = fs::remove_file(path);
                self.buf = buf;
                return Err(ApiError::new("NOT_ENOUGH_SPACE", "destination drive is full"));
            }
            if let Err(err) = file.write_all(&buf[..read]) {
                drop(file);
                let _ = fs::remove_file(path);
                self.buf = buf;
                let code = err.raw_os_error().unwrap_or(0);
                if matches!(code, 39 | 112) {
                    return Err(ApiError::new("NOT_ENOUGH_SPACE", "destination drive is full"));
                }
                self.skip_current(&ApiError::from_io("", &err));
                return Ok(());
            }
            written = written_total;
            self.progress.bytes += read as u64;
            self.progress.tick(self.current.clone());
        }
        self.buf = buf;
        drop(file);
        if declared > 0 && written != declared {
            let _ = fs::remove_file(path);
            self.skip_current(&ApiError::new(
                "CORRUPT",
                format!("expected {declared} bytes, decoded {written}"),
            ));
            return Ok(());
        }
        Ok(())
    }

    /// Records an entry that was deliberately not written (unsafe name, encrypted member).
    pub fn skip(&mut self, rel: &str, code: &str) {
        self.skipped += 1;
        self.failed_total += 1;
        self.progress.failed = self.failed_total;
        if self.failures.len() < MAX_REPORTED_FAILURES {
            self.failures.push(OpFailure {
                path: rel.to_string(),
                code: code.to_string(),
                detail: code.to_string(),
            });
        }
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Moves the temporary tree into the destination, renaming collisions instead of
    /// overwriting anything that is already there.
    pub fn merge(&mut self) -> Result<(), ApiError> {
        self.progress.phase = "merge";
        self.progress.send(None);
        let temp = self.temp.clone();
        let mut stack = vec![temp.clone()];
        while let Some(dir) = stack.pop() {
            if self.cancelled() {
                return Err(ApiError::new("CANCELED", "extraction canceled"));
            }
            let entries = fs::read_dir(&dir).map_err(|e| self.abort_io(&dir, &e))?;
            for entry in entries {
                let path = entry.map_err(|e| self.abort_io(&dir, &e))?.path();
                let meta = fs::symlink_metadata(&path)
                    .map_err(|e| self.abort_io(&path, &e))?;
                let Ok(rel) = path.strip_prefix(&temp) else {
                    continue;
                };
                let target = self.dest.join(rel);
                if meta.is_dir() {
                    fs::create_dir_all(&target).map_err(|e| self.abort_io(&target, &e))?;
                    stack.push(path);
                    continue;
                }
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|e| self.abort_io(parent, &e))?;
                }
                let target = if fs::symlink_metadata(&target).is_ok() {
                    let unique = naming::unique(&[display(&target)]);
                    PathBuf::from(unique.into_iter().next().unwrap_or_default())
                } else {
                    target
                };
                if let Err(err) = fs::rename(&path, &target) {
                    // A rename across volumes (dest on a junction) falls back to a copy.
                    if fs::copy(&path, &target).is_ok() {
                        let _ = fs::remove_file(&path);
                    } else {
                        self.skip(&display(rel), &ApiError::from_io(&display(&target), &err).code);
                    }
                }
            }
        }
        let _ = fs::remove_dir_all(&temp);
        self.merged = true;
        Ok(())
    }

    pub fn failures(&self) -> &[OpFailure] {
        &self.failures
    }

    pub fn failed_total(&self) -> u32 {
        self.failed_total
    }

    /// Entries processed so far, including the ones that were refused.
    pub fn processed(&self) -> u32 {
        self.progress.entries + self.skipped
    }

    /// Best known entry count: the declared total, or what has been processed.
    pub fn total(&self) -> u32 {
        self.progress.entries_total.max(self.processed())
    }

    pub fn bytes(&self) -> u64 {
        self.progress.bytes
    }

    /// Duplicate names inside one archive are repaired the same way collisions at the
    /// destination are; two entries must never fight for one file.
    fn unique_rel(&mut self, rel: &str) -> String {
        let key = rel.to_lowercase();
        if self.used.insert(key.clone()) {
            return rel.to_string();
        }
        let (stem, ext) = match rel.rfind('.') {
            Some(dot) if dot > rel.rfind('\\').unwrap_or(0) => (&rel[..dot], &rel[dot..]),
            _ => (rel, ""),
        };
        for n in 1..10_000 {
            let candidate = format!("{stem} ({n}){ext}");
            if self.used.insert(candidate.to_lowercase()) {
                return candidate;
            }
        }
        rel.to_string()
    }

    fn skip_current(&mut self, err: &ApiError) {
        let rel = self.current.clone().unwrap_or_default();
        self.failed_total += 1;
        self.progress.failed = self.failed_total;
        if self.failures.len() < MAX_REPORTED_FAILURES {
            self.failures.push(OpFailure {
                path: rel,
                code: err.code.clone(),
                detail: err.detail.clone(),
            });
        }
    }

    /// Turns an I/O error into an abort: the destination is gone or full, so continuing
    /// would only produce more failures.
    fn abort_io(&self, path: &Path, err: &std::io::Error) -> ApiError {
        let code = err.raw_os_error().unwrap_or(0);
        if matches!(code, 39 | 112) {
            return ApiError::new("NOT_ENOUGH_SPACE", "destination drive is full");
        }
        ApiError::from_io(&display(path), err)
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        if !self.merged {
            let _ = fs::remove_dir_all(&self.temp);
        }
    }
}

fn display(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
}

/// `std::fs` accepts long paths only when they carry the verbatim prefix, which also
/// disables the `..`/trailing-dot normalisation we already rejected names for.
pub fn verbatim(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if text.starts_with(r"\\?\") {
        return path.to_path_buf();
    }
    if let Some(rest) = text.strip_prefix(r"\\") {
        return PathBuf::from(format!(r"\\?\UNC\{rest}"));
    }
    PathBuf::from(format!(r"\\?\{text}"))
}

fn free_space(path: &Path) -> u64 {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut free = 0u64;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR(wide.as_ptr()),
            Some(&mut free),
            None,
            None,
        )
    };
    if ok.is_ok() {
        free
    } else {
        0
    }
}
