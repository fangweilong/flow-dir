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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
/// Distinguishes concurrent extractions into the same destination within this process.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

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
            succeeded: Vec::new(),
            failed_indices: Vec::new(),
            landed: Vec::new(),
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

/// Counting writer shared by [`Sink::write_with`]: every byte still passes the cancel
/// token, the free-space budget and the declared-size check, even when a backend decodes
/// straight into the destination.
pub(crate) struct Guarded<'a> {
    file: File,
    written: u64,
    declared: u64,
    budget_left: u64,
    cancel: &'a AtomicBool,
    cancelled: bool,
    over_budget: bool,
    over_size: bool,
}

struct GuardedOutcome {
    written: u64,
    cancelled: bool,
    over_budget: bool,
    over_size: bool,
}

impl Guarded<'_> {
    fn finish(self) -> GuardedOutcome {
        GuardedOutcome {
            written: self.written,
            cancelled: self.cancelled,
            over_budget: self.over_budget,
            over_size: self.over_size,
        }
    }
}

impl Write for Guarded<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            self.cancelled = true;
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "canceled"));
        }
        let next = self.written + buf.len() as u64;
        if next > self.budget_left {
            self.over_budget = true;
            return Err(std::io::Error::other("no space"));
        }
        if self.declared > 0 && next > self.declared {
            self.over_size = true;
            return Err(std::io::Error::other("declared size exceeded"));
        }
        let written = self.file.write(buf)?;
        self.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
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
        // The job id comes from the IPC caller; only path-safe characters may reach the
        // temporary directory name, or a crafted value could point at a real folder.
        let safe_job: String = job_id
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
            .take(64)
            .collect();
        let safe_job = if safe_job.is_empty() {
            std::process::id().to_string()
        } else {
            safe_job
        };
        let temp = dest.join(format!(
            ".flowdir-extract-{safe_job}-{}-{}",
            std::process::id(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
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
        // `processed()` adds the skipped entries onto this counter, so a link that never
        // reaches the cap check must not increment it: it would be reported twice.
        if link {
            self.skip(rel, "SYMLINK");
            return Ok(Take::Skip);
        }
        self.progress.entries += 1;
        if self.progress.entries > MAX_ENTRIES {
            return Err(ApiError::new(
                "TOO_MANY_ENTRIES",
                format!("archive holds more than {MAX_ENTRIES} entries"),
            ));
        }
        let rel = self.unique_rel(rel);
        let full = self.temp.join(&rel);
        self.current = Some(rel.clone());
        if is_dir {
            if let Err(err) = fs::create_dir_all(&full) {
                let error = self.abort_io(Path::new(&rel), &err);
                // A full disk makes every later entry fail too, so it still aborts. A
                // name conflict (a file already occupies this path) only affects this
                // entry: skipping it lets the rest of the archive extract.
                if error.code == "NOT_ENOUGH_SPACE" {
                    return Err(error);
                }
                self.skip_current(&error);
                return Ok(Take::Skip);
            }
            self.progress.tick(Some(rel));
            Ok(Take::Dir)
        } else {
            Ok(Take::File(full))
        }
    }

    /// Streams one file entry to the temporary tree.
    pub fn write(&mut self, path: &Path, reader: &mut dyn Read, declared: u64) -> Result<(), ApiError> {
        if let Some(parent) = path.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                let error = self.abort_io(path, &err);
                // Same policy as the directory entry itself: a full disk affects every
                // later entry and aborts, while a name conflict only skips this file.
                if error.code == "NOT_ENOUGH_SPACE" {
                    return Err(error);
                }
                self.skip_current(&error);
                return Ok(());
            }
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

    /// Streams one file entry pushed by a backend that hands over a writer instead of a
    /// reader (rar and iso decode straight into the destination). Limits, cancellation
    /// and the size checks still run through [`Guarded`], so they cannot be skipped.
    pub fn write_with<F>(&mut self, path: &Path, declared: u64, produce: F) -> Result<(), ApiError>
    where
        F: FnOnce(&mut Guarded<'_>) -> Result<(), ApiError>,
    {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| self.abort_io(path, &e))?;
        }
        let file = match File::create(path) {
            Ok(file) => file,
            Err(err) => {
                self.skip_current(&ApiError::from_io(&display(path), &err));
                return Ok(());
            }
        };
        let budget_left = self.budget.saturating_sub(self.progress.bytes);
        let mut guard = Guarded {
            file,
            written: 0,
            declared,
            budget_left,
            cancel: &self.cancel,
            cancelled: false,
            over_budget: false,
            over_size: false,
        };
        let result = produce(&mut guard);
        let outcome = guard.finish();
        self.progress.bytes += outcome.written;
        if outcome.cancelled {
            let _ = fs::remove_file(path);
            return Err(ApiError::new("CANCELED", "extraction canceled"));
        }
        if outcome.over_budget {
            let _ = fs::remove_file(path);
            return Err(ApiError::new("NOT_ENOUGH_SPACE", "destination drive is full"));
        }
        if outcome.over_size {
            let _ = fs::remove_file(path);
            return Err(ApiError::new(
                "TOO_LARGE",
                format!("{} exceeds its declared size", self.current.as_deref().unwrap_or("entry")),
            ));
        }
        if let Err(err) = result {
            let _ = fs::remove_file(path);
            return Err(err);
        }
        if declared > 0 && outcome.written != declared {
            let _ = fs::remove_file(path);
            self.skip_current(&ApiError::new(
                "CORRUPT",
                format!("expected {declared} bytes, decoded {}", outcome.written),
            ));
            return Ok(());
        }
        self.progress.tick(self.current.clone());
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
        // Each stack entry carries the destination folder its children belong to: a
        // directory that had to be renamed (collision, junction) must not send its
        // children back through the original name.
        let mut stack = vec![(temp.clone(), self.dest.clone())];
        while let Some((dir, dest_dir)) = stack.pop() {
            if self.cancelled() {
                return Err(ApiError::new("CANCELED", "extraction canceled"));
            }
            let entries = fs::read_dir(&dir).map_err(|e| self.abort_io(&dir, &e))?;
            for entry in entries {
                let path = entry.map_err(|e| self.abort_io(&dir, &e))?.path();
                let meta = fs::symlink_metadata(&path)
                    .map_err(|e| self.abort_io(&path, &e))?;
                let name = match path.file_name() {
                    Some(name) => name.to_os_string(),
                    None => continue,
                };
                let rel = path.strip_prefix(&temp).unwrap_or(&path);
                let target = dest_dir.join(&name);
                if meta.is_dir() {
                    let target = merge_dir_target(&target);
                    fs::create_dir_all(&target).map_err(|e| self.abort_io(&target, &e))?;
                    stack.push((path, target));
                    continue;
                }
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|e| self.abort_io(parent, &e))?;
                }
                let target = if fs::symlink_metadata(&target).is_ok() {
                    // Keep the `\\?\` form: `display()` strips it, which would turn a UNC
                    // target into a relative `UNC\server\share\...` path.
                    let unique = naming::unique(&[target.to_string_lossy().into_owned()], &[]);
                    PathBuf::from(unique.into_iter().next().unwrap_or_default())
                } else {
                    target
                };
                let mut target = target;
                let mut tries = 0;
                loop {
                    match move_no_replace(&path, &target) {
                        Ok(()) => break,
                        // The name check above is a snapshot: another extraction or the user
                        // can create it between the check and the move.
                        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && tries < 16 => {
                            tries += 1;
                            let unique =
                                naming::unique(&[target.to_string_lossy().into_owned()], &[]);
                            target = PathBuf::from(unique.into_iter().next().unwrap_or_default());
                        }
                        Err(err) => {
                            self.skip(
                                &display(rel),
                                &ApiError::from_io(&display(&target), &err).code,
                            );
                            break;
                        }
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

/// Destination folder an archive directory may merge into.
///
/// A plain existing folder is reused. Anything else — a file with that name, a junction or
/// a symlink — would either abort the merge (`create_dir_all` fails) or redirect every
/// child outside the chosen destination, so the folder is given a free name instead.
/// Moves `src` to `dst` without replacing an existing destination. A hard link is the
/// atomic no-replace primitive on Windows; the copy fallback uses an exclusive create so it
/// cannot silently overwrite either.
fn move_no_replace(src: &Path, dst: &Path) -> std::io::Result<()> {
    match fs::hard_link(src, dst) {
        Ok(()) => {
            let _ = fs::remove_file(src);
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Err(err),
        Err(_) => {
            let mut source = fs::File::open(src)?;
            let mut target = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dst)?;
            match std::io::copy(&mut source, &mut target) {
                Ok(_) => {
                    drop(target);
                    let _ = fs::remove_file(src);
                    Ok(())
                }
                Err(err) => {
                    drop(target);
                    let _ = fs::remove_file(dst);
                    Err(err)
                }
            }
        }
    }
}

fn merge_dir_target(target: &Path) -> PathBuf {
    let Ok(meta) = fs::symlink_metadata(target) else {
        return target.to_path_buf();
    };
    if meta.is_dir() && !is_reparse_point(&meta) {
        return target.to_path_buf();
    }
    let unique = naming::unique(&[target.to_string_lossy().into_owned()], &[]);
    PathBuf::from(unique.into_iter().next().unwrap_or_default())
}

/// Reparse points (mount points included) are reported as symlinks only by some std
/// versions, so the attribute is checked directly.
fn is_reparse_point(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
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
