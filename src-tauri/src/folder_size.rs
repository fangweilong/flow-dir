//! One-shot recursive size for the active tab. The walk is cancellable through the shared
//! `hash::ToolState` job map (`tool_cancel`), and reparse-point directories are skipped so
//! a junction cannot loop the scan or cross onto another volume.

use std::os::windows::fs::MetadataExt;
use std::path::PathBuf;

use serde::Serialize;
use tauri::ipc::Channel;

use crate::error::ApiError;
use crate::hash::ToolState;

const ATTR_REPARSE_POINT: u32 = 0x400;
const ATTR_OFFLINE: u32 = 0x1000;
const ATTR_RECALL_ON_OPEN: u32 = 0x4_0000;
const ATTR_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
const ATTR_CLOUD: u32 = ATTR_OFFLINE | ATTR_RECALL_ON_OPEN | ATTR_RECALL_ON_DATA_ACCESS;
/// Rows between progress messages; the UI only needs a moving counter.
const REPORT_EVERY: u64 = 4096;

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct FolderSizeProgress {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub done: bool,
    pub aborted: bool,
    pub error: Option<ApiError>,
}

/// Directory aliases (junctions/symlinks) are skipped; cloud placeholders keep their
/// reparse bit but must still be counted, so the cloud attribute exempts them.
fn is_link_dir(attrs: u32) -> bool {
    attrs & ATTR_REPARSE_POINT != 0 && attrs & ATTR_CLOUD == 0
}

pub fn start(
    state: &ToolState,
    job_id: String,
    path: String,
    on_event: Channel<FolderSizeProgress>,
) -> Result<(), ApiError> {
    let root = PathBuf::from(&path);
    if path.is_empty() || path.len() > 1024 || !root.is_absolute() {
        return Err(ApiError::new("INVALID_PATH", "path must be absolute"));
    }
    if !root.is_dir() {
        return Err(ApiError::new("NOT_DIR", "not a directory"));
    }
    let job = state.begin(&job_id);
    let jobs = state.clone();
    let job_key = job_id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut progress = FolderSizeProgress::default();
        let mut stack = vec![root];
        let mut since_report = 0u64;
        let mut aborted = false;
        'walk: while let Some(dir) = stack.pop() {
            if job.stopped() {
                aborted = true;
                break;
            }
            let Ok(reader) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in reader.flatten() {
                if job.stopped() {
                    aborted = true;
                    break 'walk;
                }
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                let attrs = meta.file_attributes();
                if meta.is_dir() {
                    if is_link_dir(attrs) {
                        continue;
                    }
                    progress.dirs += 1;
                    stack.push(entry.path());
                } else if meta.is_file() {
                    progress.files += 1;
                    progress.bytes = progress.bytes.saturating_add(meta.len());
                } else {
                    continue;
                }
                since_report += 1;
                if since_report >= REPORT_EVERY {
                    since_report = 0;
                    let _ = on_event.send(progress.clone());
                }
            }
        }
        progress.aborted = aborted;
        progress.done = true;
        let _ = on_event.send(progress);
        jobs.finish(&job_key);
    });
    Ok(())
}