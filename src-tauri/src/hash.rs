//! Streaming CNG hashing plus the shared job state behind the checksum and duplicate
//! tools. Both features are on-demand: nothing here runs until the user asks for it.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::ipc::Channel;
use windows::core::PCWSTR;
use windows::Win32::Security::Cryptography::{
    BCryptCloseAlgorithmProvider, BCryptCreateHash, BCryptDestroyHash, BCryptFinishHash,
    BCryptHashData, BCryptOpenAlgorithmProvider, BCRYPT_ALG_HANDLE, BCRYPT_HASH_HANDLE,
    BCRYPT_MD5_ALGORITHM, BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS, BCRYPT_SHA1_ALGORITHM,
    BCRYPT_SHA256_ALGORITHM,
};

use crate::error::ApiError;

/// Read size per `BCryptHashData` call; large enough to keep the CNG overhead negligible,
/// small enough that pause/cancel and the speed limit stay responsive.
const CHUNK: usize = 256 * 1024;
/// Progress messages per second, independent of how fast the files are read.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(120);

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HashResult {
    pub path: String,
    pub algorithm: String,
    pub hash: String,
    pub size: u64,
    pub error: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HashProgress {
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
    pub results: Vec<HashResult>,
}

/// One running checksum/duplicate job. `paused` is a second switch next to `cancel` so the
/// user can park a scan that is hitting the disk too hard without losing its progress.
pub struct Job {
    pub cancel: AtomicBool,
    pub paused: AtomicBool,
    /// Bytes read so far, shared by the duplicate worker threads for the speed readout.
    pub bytes: AtomicU64,
}

impl Job {
    pub fn stopped(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Blocks while the job is paused; returns `true` when the job was cancelled meanwhile.
    pub fn wait_if_paused(&self) -> bool {
        while self.paused.load(Ordering::Relaxed) {
            if self.stopped() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(60));
        }
        self.stopped()
    }
}

/// Jobs are keyed by the id the frontend mints, so several windows (or a cancelled-then-
/// restarted scan) never share a token.
#[derive(Clone, Default)]
pub struct ToolState {
    jobs: Arc<Mutex<HashMap<String, Arc<Job>>>>,
}

impl ToolState {
    pub fn begin(&self, job_id: &str) -> Arc<Job> {
        let job = Arc::new(Job {
            cancel: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            bytes: AtomicU64::new(0),
        });
        self.jobs
            .lock()
            .unwrap()
            .insert(job_id.to_string(), job.clone());
        job
    }

    pub fn cancel(&self, job_id: &str) {
        if let Some(job) = self.jobs.lock().unwrap().get(job_id) {
            job.cancel.store(true, Ordering::Relaxed);
            job.paused.store(false, Ordering::Relaxed);
        }
    }

    pub fn pause(&self, job_id: &str, paused: bool) {
        if let Some(job) = self.jobs.lock().unwrap().get(job_id) {
            job.paused.store(paused, Ordering::Relaxed);
        }
    }

    pub fn finish(&self, job_id: &str) {
        self.jobs.lock().unwrap().remove(job_id);
    }

    /// Ids still running; quitting warns about them like it does for file operations.
    pub fn active(&self) -> Vec<String> {
        self.jobs.lock().unwrap().keys().cloned().collect()
    }
}

/// Streaming hash over one algorithm id (`sha256`, `sha1`, `md5`).
pub(crate) struct Hasher {
    alg: BCRYPT_ALG_HANDLE,
    hash: BCRYPT_HASH_HANDLE,
    len: usize,
}

impl Hasher {
    pub fn new(algorithm: &str) -> Result<Self, ApiError> {
        let (alg_id, len) = match algorithm {
            "md5" => (BCRYPT_MD5_ALGORITHM, 16usize),
            "sha1" => (BCRYPT_SHA1_ALGORITHM, 20usize),
            _ => (BCRYPT_SHA256_ALGORITHM, 32usize),
        };
        unsafe {
            let mut alg = BCRYPT_ALG_HANDLE(std::ptr::null_mut());
            let opened = BCryptOpenAlgorithmProvider(
                &mut alg,
                alg_id,
                PCWSTR::null(),
                BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0),
            );
            if opened.0 < 0 {
                return Err(ApiError::new("HASH", format!("BCryptOpenAlgorithmProvider 0x{:08X}", opened.0)));
            }
            let mut hash = BCRYPT_HASH_HANDLE(std::ptr::null_mut());
            let created = BCryptCreateHash(alg, &mut hash, None, None, 0);
            if created.0 < 0 {
                let _ = BCryptCloseAlgorithmProvider(alg, 0);
                return Err(ApiError::new("HASH", format!("BCryptCreateHash 0x{:08X}", created.0)));
            }
            Ok(Self { alg, hash, len })
        }
    }

    pub fn update(&mut self, bytes: &[u8]) -> Result<(), ApiError> {
        let status = unsafe { BCryptHashData(self.hash, bytes, 0) };
        if status.0 < 0 {
            return Err(ApiError::new("HASH", format!("BCryptHashData 0x{:08X}", status.0)));
        }
        Ok(())
    }

    pub fn finish(self) -> Result<String, ApiError> {
        let mut digest = vec![0u8; self.len];
        let status = unsafe { BCryptFinishHash(self.hash, &mut digest, 0) };
        if status.0 < 0 {
            return Err(ApiError::new("HASH", format!("BCryptFinishHash 0x{:08X}", status.0)));
        }
        Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
    }
}

impl Drop for Hasher {
    fn drop(&mut self) {
        unsafe {
            let _ = BCryptDestroyHash(self.hash);
            let _ = BCryptCloseAlgorithmProvider(self.alg, 0);
        }
    }
}

/// One-shot digest for the small head/tail samples used by the duplicate pre-pass.
pub(crate) fn sha256_bytes(bytes: &[u8]) -> Result<String, ApiError> {
    let mut hasher = Hasher::new("sha256")?;
    hasher.update(bytes)?;
    hasher.finish()
}

/// Token bucket shared by the duplicate workers. `rate == 0` disables the limit.
pub(crate) struct Throttle {
    rate: u64,
    /// Earliest instant the next read may start. Guarded by its own lock, but the sleep
    /// happens outside it, so workers queue their slots instead of serialising the reads.
    next: Mutex<Instant>,
}

impl Throttle {
    pub fn new(rate: u64) -> Self {
        Self {
            rate,
            next: Mutex::new(Instant::now()),
        }
    }

    pub fn acquire(&self, bytes: u64) {
        if self.rate == 0 {
            return;
        }
        let slot = Duration::from_secs_f64(bytes as f64 / self.rate as f64);
        let wait = {
            let mut next = self.next.lock().unwrap();
            let now = Instant::now();
            let start = (*next).max(now);
            *next = start + slot;
            start.saturating_duration_since(now)
        };
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
    }
}

/// Hashes `path` without ever holding more than one chunk in memory. `on_bytes` receives
/// the cumulative byte count so both tools can drive the progress and speed readout.
pub(crate) fn hash_file(
    path: &Path,
    algorithm: &str,
    job: &Job,
    throttle: Option<&Throttle>,
    mut on_bytes: impl FnMut(u64),
) -> Result<String, ApiError> {
    let mut file = File::open(path).map_err(|e| ApiError::from_io(&path.display().to_string(), &e))?;
    let mut hasher = Hasher::new(algorithm)?;
    let mut buffer = vec![0u8; CHUNK];
    let mut read_total = 0u64;
    loop {
        if job.wait_if_paused() {
            return Err(ApiError::new("CANCELED", "cancelled"));
        }
        let read = file
            .read(&mut buffer)
            .map_err(|e| ApiError::from_io(&path.display().to_string(), &e))?;
        if read == 0 {
            break;
        }
        if let Some(throttle) = throttle {
            throttle.acquire(read as u64);
        }
        hasher.update(&buffer[..read])?;
        read_total += read as u64;
        job.bytes.fetch_add(read as u64, Ordering::Relaxed);
        on_bytes(read_total);
    }
    hasher.finish()
}

/// Starts a checksum job over `paths`; returns immediately and streams progress.
pub fn start_hash(
    state: &ToolState,
    job_id: String,
    paths: Vec<String>,
    algorithm: String,
    progress: Channel<HashProgress>,
) -> Result<(), ApiError> {
    let algorithm = match algorithm.as_str() {
        "md5" | "sha1" | "sha256" => algorithm,
        other => return Err(ApiError::new("BAD_ALGORITHM", format!("unsupported {other}"))),
    };
    let job = state.begin(&job_id);
    let state = state.clone();
    let total = paths.len() as u32;
    std::thread::Builder::new()
        .name(format!("flowdir-hash-{job_id}"))
        .spawn(move || {
            let sizes: Vec<u64> = paths
                .iter()
                .map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
                .collect();
            let bytes_total: u64 = sizes.iter().sum();
            let mut results = Vec::with_capacity(paths.len());
            let mut finished = 0u32;
            let mut sent = Instant::now() - PROGRESS_INTERVAL;
            let mut aborted = false;
            for (index, path) in paths.iter().enumerate() {
                if job.stopped() {
                    aborted = true;
                    break;
                }
                let base = job.bytes.load(Ordering::Relaxed);
                let outcome = hash_file(Path::new(path), &algorithm, &job, None, |read| {
                    if sent.elapsed() >= PROGRESS_INTERVAL {
                        sent = Instant::now();
                        let _ = progress.send(HashProgress {
                            job_id: job_id.clone(),
                            phase: "hash".into(),
                            current: finished,
                            total,
                            name: Some(path.clone()),
                            bytes: base + read,
                            bytes_total,
                            paused: job.paused.load(Ordering::Relaxed),
                            done: false,
                            aborted: false,
                            error: None,
                            results: Vec::new(),
                        });
                    }
                });
                finished += 1;
                if let Err(err) = &outcome {
                    if err.code == "CANCELED" {
                        aborted = true;
                        break;
                    }
                }
                results.push(HashResult {
                    path: path.clone(),
                    algorithm: algorithm.clone(),
                    hash: outcome.as_ref().map(|h| h.clone()).unwrap_or_default(),
                    size: sizes[index],
                    error: outcome.err().map(|e| e.detail),
                });
            }
            let _ = progress.send(HashProgress {
                job_id: job_id.clone(),
                phase: "done".into(),
                current: finished,
                total,
                name: None,
                bytes: job.bytes.load(Ordering::Relaxed),
                bytes_total,
                paused: false,
                done: true,
                aborted,
                error: None,
                results,
            });
            state.finish(&job_id);
        })
        .map(|_| ())
        .map_err(|e| ApiError::new("THREAD", e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_sha256_matches_known_vector() {
        let mut hasher = Hasher::new("sha256").unwrap();
        hasher.update(b"a").unwrap();
        hasher.update(b"bc").unwrap();
        assert_eq!(
            hasher.finish().unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn md5_matches_known_vector() {
        assert_eq!(sha256_bytes(b"").unwrap().len(), 64);
        let mut hasher = Hasher::new("md5").unwrap();
        hasher.update(b"abc").unwrap();
        assert_eq!(hasher.finish().unwrap(), "900150983cd24fb0d6963f7d28e17f72");
    }
}