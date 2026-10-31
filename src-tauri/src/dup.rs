//! On-demand duplicate finder. Phase 1 groups by size, phase 2 samples the head and tail
//! of each candidate, phase 3 hashes only the files that still collide. Nothing is
//! indexed and every phase obeys the same cancel/pause switch.

use std::collections::HashMap;
use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::Serialize;
use tauri::ipc::Channel;

use crate::error::ApiError;
use crate::hash::{hash_file, sha256_bytes, Job, Throttle, ToolState};

/// Sample size per end for the quick pre-pass.
const SAMPLE: u64 = 64 * 1024;
/// Directories that are never worth walking: the recycle bin is not a user folder and the
/// volume metadata cannot be read sanely.
const ALWAYS_EXCLUDED: &[&str] = &["$recycle.bin", "system volume information"];

const ATTR_HIDDEN: u32 = 0x0000_0002;
const ATTR_SYSTEM: u32 = 0x0000_0004;
const ATTR_OFFLINE: u32 = 0x0000_1000;
const ATTR_RECALL_ON_OPEN: u32 = 0x0004_0000;
const ATTR_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
const ATTR_REPARSE_POINT: u32 = 0x0000_0400;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DupEntry {
    pub path: String,
    pub size: u64,
    /// Modification time in milliseconds since the Unix epoch.
    pub mtime: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DupGroup {
    pub size: u64,
    pub hash: String,
    pub files: Vec<DupEntry>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DupProgress {
    pub job_id: String,
    pub phase: String,
    pub current: u32,
    pub total: u32,
    pub name: Option<String>,
    pub bytes: u64,
    pub bytes_total: u64,
    pub paused: bool,
    pub done: bool,
    pub aborted: bool,
    pub error: Option<ApiError>,
    pub groups: Vec<DupGroup>,
    pub scanned: u32,
    pub skipped_hidden: u32,
    pub skipped_cloud: u32,
    pub skipped_hardlink: u32,
    pub elapsed_ms: u64,
}

#[derive(Clone)]
struct Candidate {
    path: PathBuf,
    size: u64,
    mtime: u64,
}

struct ScanStats {
    scanned: u32,
    skipped_hidden: u32,
    skipped_cloud: u32,
    skipped_hardlink: u32,
}

/// Cheap attribute screen shared by the walker and the hardlink pass. Directories with a
/// reparse bit are skipped entirely so junctions cannot loop or cross onto another volume.
fn is_special(meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    let attrs = meta.file_attributes();
    attrs & (ATTR_REPARSE_POINT | ATTR_OFFLINE | ATTR_RECALL_ON_OPEN | ATTR_RECALL_ON_DATA_ACCESS) != 0
}

fn is_hidden_or_system(meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & (ATTR_HIDDEN | ATTR_SYSTEM) != 0
}

fn mtime_ms(meta: &Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn link_count(path: &Path) -> u32 {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
    let Ok(file) = File::open(path) else {
        return 1;
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    let okay = unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) };
    if okay.is_ok() {
        info.nNumberOfLinks
    } else {
        1
    }
}

fn excluded_dir(name: &str, exclude: &[String]) -> bool {
    let lower = name.to_ascii_lowercase();
    ALWAYS_EXCLUDED.contains(&lower.as_str())
        || exclude.iter().any(|entry| {
            let entry = entry.trim().trim_end_matches(['\\', '/']).to_ascii_lowercase();
            !entry.is_empty() && (lower == entry || lower == entry.split('\\').next_back().unwrap_or(""))
        })
}

fn send_progress(
    progress: &Channel<DupProgress>,
    job_id: &str,
    phase: &str,
    current: u32,
    total: u32,
    name: Option<String>,
    job: &Job,
    bytes_total: u64,
    stats: &ScanStats,
    started: Instant,
) {
    let _ = progress.send(DupProgress {
        job_id: job_id.to_string(),
        phase: phase.to_string(),
        current,
        total,
        name,
        bytes: job.bytes.load(Ordering::Relaxed),
        bytes_total,
        paused: job.paused.load(Ordering::Relaxed),
        done: false,
        aborted: false,
        error: None,
        groups: Vec::new(),
        scanned: stats.scanned,
        skipped_hidden: stats.skipped_hidden,
        skipped_cloud: stats.skipped_cloud,
        skipped_hardlink: stats.skipped_hardlink,
        elapsed_ms: started.elapsed().as_millis() as u64,
    });
}

/// Recursively collects regular files. Errors are skipped: one unreadable folder must not
/// abort a scan of the whole drive.
fn walk(
    roots: &[String],
    include_hidden: bool,
    min_size: u64,
    exclude: &[String],
    job: &Job,
    stats: &mut ScanStats,
    progress: &Channel<DupProgress>,
    job_id: &str,
    started: Instant,
    files: &mut Vec<Candidate>,
) {
    let mut stack: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();
    let mut last = Instant::now();
    while let Some(dir) = stack.pop() {
        if job.stopped() {
            return;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if job.stopped() {
                return;
            }
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if excluded_dir(&name, exclude) {
                    continue;
                }
                if !include_hidden && is_hidden_or_system(&meta) {
                    stats.skipped_hidden += 1;
                    continue;
                }
                if is_special(&meta) {
                    stats.skipped_cloud += 1;
                    continue;
                }
                stack.push(path);
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            if is_special(&meta) {
                stats.skipped_cloud += 1;
                continue;
            }
            if meta.file_attributes() & ATTR_REPARSE_POINT != 0 {
                continue;
            }
            if !include_hidden && is_hidden_or_system(&meta) {
                stats.skipped_hidden += 1;
                continue;
            }
            // `min_size == 0` means "no filter"; forcing the floor to 1 silently
            // hid all empty-file duplicates.
            if meta.len() < min_size {
                continue;
            }
            stats.scanned += 1;
            files.push(Candidate {
                path,
                size: meta.len(),
                mtime: mtime_ms(&meta),
            });
            if last.elapsed() >= Duration::from_millis(150) {
                last = Instant::now();
                send_progress(progress, job_id, "scan", files.len() as u32, 0, None, job, 0, stats, started);
            }
        }
    }
}

/// Head + tail digest. Including the size in the key is unnecessary because callers only
/// compare within one size group, but it keeps a standalone digest honest.
fn quick_hash(candidate: &Candidate, job: &Job, throttle: &Throttle) -> Result<String, ApiError> {
    if job.wait_if_paused() {
        return Err(ApiError::new("CANCELED", "cancelled"));
    }
    let mut file = File::open(&candidate.path)
        .map_err(|e| ApiError::from_io(&candidate.path.display().to_string(), &e))?;
    let head_len = candidate.size.min(SAMPLE) as usize;
    let mut sample = vec![0u8; head_len];
    let read = file
        .read(&mut sample)
        .map_err(|e| ApiError::from_io(&candidate.path.display().to_string(), &e))?;
    sample.truncate(read);
    // A small file was already read completely; only large files need the tail seek.
    if candidate.size > SAMPLE * 2 {
        file.seek(SeekFrom::End(-(SAMPLE as i64)))
            .map_err(|e| ApiError::from_io(&candidate.path.display().to_string(), &e))?;
        let mut tail = vec![0u8; SAMPLE as usize];
        let read = file
            .read(&mut tail)
            .map_err(|e| ApiError::from_io(&candidate.path.display().to_string(), &e))?;
        tail.truncate(read);
        sample.extend_from_slice(&tail);
    }
    throttle.acquire(sample.len() as u64);
    job.bytes.fetch_add(sample.len() as u64, Ordering::Relaxed);
    sha256_bytes(&sample)
}

/// Runs `work` across `workers` threads over `items`, returning an aligned result vector.
/// Progress is polled here rather than pushed from workers, so the channel is written by a
/// single thread and the UI cannot receive interleaved counts.
fn parallel_map<T, R, F>(
    items: &[T],
    workers: usize,
    job: &Job,
    progress: &Channel<DupProgress>,
    job_id: &str,
    phase: &str,
    total: u32,
    stats: &ScanStats,
    bytes_total: u64,
    started: Instant,
    work: F,
) -> Vec<Option<R>>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> Result<R, ApiError> + Sync,
{
    let slots: Vec<Mutex<Option<R>>> = items.iter().map(|_| Mutex::new(None)).collect();
    let index = AtomicU32::new(0);
    let remaining = AtomicU32::new(items.len() as u32);
    let workers = workers.max(1).min(items.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                if job.stopped() {
                    break;
                }
                let next = index.fetch_add(1, Ordering::Relaxed) as usize;
                if next >= items.len() {
                    break;
                }
                let result = work(&items[next]).ok();
                *slots[next].lock().unwrap() = result;
                remaining.fetch_sub(1, Ordering::Relaxed);
            });
        }
        // The scope cannot report progress while blocked on a join, so poll until the
        // workers drain; a cancelled job exits the loop immediately.
        while remaining.load(Ordering::Relaxed) > 0 && !job.stopped() {
            std::thread::sleep(Duration::from_millis(120));
            send_progress(
                progress,
                job_id,
                phase,
                total - remaining.load(Ordering::Relaxed),
                total,
                None,
                job,
                bytes_total,
                stats,
                started,
            );
        }
    });
    slots.into_iter().map(|slot| slot.into_inner().unwrap()).collect()
}

fn group_results(items: &[Candidate], hashes: &[Option<String>]) -> HashMap<(u64, String), Vec<usize>> {
    let mut groups: HashMap<(u64, String), Vec<usize>> = HashMap::new();
    for (index, hash) in hashes.iter().enumerate() {
        if let Some(hash) = hash {
            groups
                .entry((items[index].size, hash.clone()))
                .or_default()
                .push(index);
        }
    }
    groups.retain(|_, members| members.len() > 1);
    groups
}

fn default_workers(configured: u32) -> usize {
    if configured > 0 {
        return configured.min(16) as usize;
    }
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    (cores / 2).clamp(1, 4)
}

/// Starts the three-phase scan; returns immediately and streams progress.
pub fn start(
    state: &ToolState,
    job_id: String,
    roots: Vec<String>,
    include_hidden: bool,
    min_size: u64,
    concurrency: u32,
    max_bytes_per_sec: u64,
    exclude: Vec<String>,
    progress: Channel<DupProgress>,
) -> Result<(), ApiError> {
    if roots.is_empty() {
        return Err(ApiError::new("EMPTY", "no paths"));
    }
    for root in &roots {
        if !Path::new(root).is_dir() {
            return Err(ApiError::new("NOT_FOUND", root.clone()));
        }
    }
    let job = state.begin(&job_id);
    let state = state.clone();
    std::thread::Builder::new()
        .name(format!("flowdir-dup-{job_id}"))
        .spawn(move || {
            let started = Instant::now();
            let mut stats = ScanStats {
                scanned: 0,
                skipped_hidden: 0,
                skipped_cloud: 0,
                skipped_hardlink: 0,
            };
            let mut files = Vec::new();
            walk(
                &roots,
                include_hidden,
                min_size,
                &exclude,
                &job,
                &mut stats,
                &progress,
                &job_id,
                started,
                &mut files,
            );
            let aborted = job.stopped();
            let throttle = Throttle::new(max_bytes_per_sec);
            let workers = default_workers(concurrency);
            let mut groups: Vec<DupGroup> = Vec::new();
            if !aborted {
                // Only files whose size is shared can ever be duplicates.
                let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
                for (index, candidate) in files.iter().enumerate() {
                    by_size.entry(candidate.size).or_default().push(index);
                }
                let mut candidates: Vec<Candidate> = by_size
                    .values()
                    .filter(|indices| indices.len() > 1)
                    .flat_map(|indices| indices.iter().map(|&i| files[i].clone()))
                    .collect();

                // Hard links share their bytes, so reporting them as duplicates would
                // offer a delete that frees nothing.
                let mut linked = Vec::with_capacity(candidates.len());
                for candidate in candidates {
                    if link_count(&candidate.path) > 1 {
                        stats.skipped_hardlink += 1;
                    } else {
                        linked.push(candidate);
                    }
                }
                candidates = linked;

                let sample_bytes: u64 = candidates
                    .iter()
                    .map(|c| c.size.clamp(1, SAMPLE * 2))
                    .sum();
                let quick: Vec<Option<String>> = parallel_map(
                        &candidates,
                        workers,
                        &job,
                        &progress,
                        &job_id,
                        "quick",
                        candidates.len() as u32,
                        &stats,
                        sample_bytes,
                        started,
                        |candidate| quick_hash(candidate, &job, &throttle),
                    );
                let quick_groups = group_results(&candidates, &quick);
                let survivors: Vec<Candidate> = quick_groups
                    .values()
                    .flat_map(|indices| indices.iter().map(|&i| candidates[i].clone()))
                    .collect();
                let full_bytes: u64 = survivors.iter().map(|c| c.size).sum();
                let full: Vec<Option<String>> = parallel_map(
                    &survivors,
                    workers,
                    &job,
                    &progress,
                    &job_id,
                    "full",
                    survivors.len() as u32,
                    &stats,
                    full_bytes,
                    started,
                    |candidate| hash_file(&candidate.path, "sha256", &job, Some(&throttle), |_| {}),
                );
                for ((size, hash), indices) in group_results(&survivors, &full) {
                    groups.push(DupGroup {
                        size,
                        hash,
                        files: indices
                            .iter()
                            .map(|&i| {
                                let candidate = &survivors[i];
                                DupEntry {
                                    path: candidate.path.display().to_string(),
                                    size: candidate.size,
                                    mtime: candidate.mtime,
                                }
                            })
                            .collect(),
                    });
                }
                // Biggest reclaimable space first; ties keep a stable path order.
                groups.sort_by(|a, b| {
                    let waste = |g: &DupGroup| g.size * (g.files.len() as u64 - 1);
                    waste(b).cmp(&waste(a)).then_with(|| a.files[0].path.cmp(&b.files[0].path))
                });
            }
            let _ = progress.send(DupProgress {
                job_id: job_id.clone(),
                phase: "done".into(),
                current: stats.scanned,
                total: stats.scanned,
                name: None,
                bytes: job.bytes.load(Ordering::Relaxed),
                bytes_total: job.bytes.load(Ordering::Relaxed),
                paused: false,
                done: true,
                aborted: aborted || job.stopped(),
                error: None,
                groups,
                scanned: stats.scanned,
                skipped_hidden: stats.skipped_hidden,
                skipped_cloud: stats.skipped_cloud,
                skipped_hardlink: stats.skipped_hardlink,
                elapsed_ms: started.elapsed().as_millis() as u64,
            });
            state.finish(&job_id);
        })
        .map(|_| ())
        .map_err(|e| ApiError::new("THREAD", e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(path: &str, size: u64) -> Candidate {
        Candidate {
            path: PathBuf::from(path),
            size,
            mtime: 0,
        }
    }

    #[test]
    fn grouping_keeps_only_collisions() {
        let files = [
            candidate("a", 10),
            candidate("b", 10),
            candidate("c", 20),
            candidate("d", 20),
            candidate("e", 20),
        ];
        let hashes = [
            Some("same".to_string()),
            Some("same".to_string()),
            Some("x".to_string()),
            Some("y".to_string()),
            Some("y".to_string()),
        ];
        let groups = group_results(&files, &hashes);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups.get(&(10, "same".to_string())).unwrap(), &vec![0, 1]);
        assert_eq!(groups.get(&(20, "y".to_string())).unwrap(), &vec![3, 4]);
        // A file that failed to hash must not join a group.
        let missing = group_results(&files, &[None, Some("same".into()), None, None, None]);
        assert!(missing.is_empty());
    }

    #[test]
    fn excluded_directory_matches_case_insensitively() {
        let exclude = vec!["Node_Modules".to_string()];
        assert!(excluded_dir("node_modules", &exclude));
        assert!(excluded_dir("$Recycle.Bin", &exclude));
        assert!(!excluded_dir("src", &exclude));
    }
}