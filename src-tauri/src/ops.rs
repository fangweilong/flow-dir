//! Shell file operations through `IFileOperation`.
//!
//! Every operation runs on its own STA thread: `IFileOperation` requires a
//! single-threaded apartment and `PerformOperations` blocks until the shell is
//! done, so a dispatcher thread would serialize unrelated copies for no gain.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use windows::core::{implement, HRESULT, Result as WinResult, HSTRING, PCWSTR};
use windows::Win32::Foundation::{E_ABORT, HWND};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{
    FileOperation, IFileOperation, IFileOperationProgressSink, IFileOperationProgressSink_Impl,
    IShellItem, SHCreateItemFromParsingName, FOFX_RECYCLEONDELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION,
    FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, SIGDN_FILESYSPATH,
};

use crate::error::ApiError;
use crate::protect;

const PROGRESS_INTERVAL_MS: u128 = 80;
const PHASES: [&str; 6] = ["start", "copy", "move", "delete", "rename", "newdir"];
/// Failures are listed for the user to retry; a mass failure must not grow the message.
const MAX_REPORTED_FAILURES: usize = 200;

/// One item the shell refused, with the HRESULT mapped to an app error code.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OpFailure {
    pub path: String,
    pub code: String,
    pub detail: String,
}

/// Where a source actually landed, as reported by the shell. A conflict resolved with
/// "keep both" creates `name (1).ext`; the undo entry must target that file, not the
/// requested name (which may be a pre-existing file the operation did not create).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LandedItem {
    pub index: u32,
    pub path: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OpProgress {
    pub op_id: String,
    pub phase: String,
    pub current: u32,
    pub total: u32,
    pub name: Option<String>,
    pub work: u64,
    pub work_total: u64,
    /// Failures seen so far; kept live so the card can show them while the op runs.
    pub failed_count: u32,
    /// Failure details, sent once with the final message.
    pub failed: Vec<OpFailure>,
    /// Source indices that completed, sent only with the final message of an aborted
    /// operation so the UI can still undo the part that reached the disk.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub succeeded: Vec<u32>,
    /// Indices of failed sources, complete even when `failed` is capped; the elevated
    /// retry uses them to replay exactly the denied subset.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_indices: Vec<u32>,
    /// Actual destination of each completed source; see [`LandedItem`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub landed: Vec<LandedItem>,
    pub done: bool,
    pub aborted: bool,
    pub error: Option<ApiError>,
}

/// `IFileOperation::PerformOperations` returns `S_OK` when individual items failed; the
/// per-item HRESULT arrives in the sink callbacks. This log is shared between the sink and
/// the operation thread so the final progress message can carry the failures.
#[derive(Default)]
struct FailureLog {
    total: AtomicU32,
    items: Mutex<Vec<OpFailure>>,
    /// Set by any `ACCESS_DENIED`, including failures past the report cap. The retry
    /// button must not depend on the capped `items` list.
    denied: std::sync::atomic::AtomicBool,
}

impl FailureLog {
    fn record(&self, path: &str, hr: HRESULT) {
        if hr.is_err() {
            self.push(path, ApiError::from_windows(path, hr.0));
        }
    }

    /// Queue-time failures already carry a mapped {@link ApiError}.
    fn push(&self, path: &str, err: ApiError) {
        self.total.fetch_add(1, Ordering::Relaxed);
        if err.code == "ACCESS_DENIED" {
            self.denied.store(true, Ordering::Relaxed);
        }
        let mut items = self.items.lock().unwrap();
        if items.len() < MAX_REPORTED_FAILURES {
            items.push(OpFailure {
                path: path.to_string(),
                code: err.code,
                detail: err.detail,
            });
        }
    }

    fn denied(&self) -> bool {
        self.denied.load(Ordering::Relaxed)
    }

    fn count(&self) -> u32 {
        self.total.load(Ordering::Relaxed)
    }

    fn take(&self) -> Vec<OpFailure> {
        std::mem::take(&mut *self.items.lock().unwrap())
    }
}

/// What the operation thread reports back after `PerformOperations` returns.
#[derive(Debug)]
pub(crate) struct Outcome {
    pub(crate) aborted: bool,
    pub(crate) error: Option<ApiError>,
    /// Detailed entries, capped at {@link MAX_REPORTED_FAILURES}.
    pub(crate) failed: Vec<OpFailure>,
    /// Every failure, including the ones cut from `failed`.
    pub(crate) failed_total: u32,
    /// Source indices the shell completed, in completion order. Only an aborted batch
    /// exposes them: the UI records an undo for what landed, which it cannot derive from
    /// `failed` because that list is capped.
    pub(crate) succeeded: Vec<u32>,
    /// Source indices that failed, uncapped (4 bytes each; the 200-entry cap exists for
    /// the user-facing message, not for this bookkeeping).
    pub(crate) failed_indices: Vec<u32>,
    /// Actual destination paths reported by the shell, used to build undo entries.
    pub(crate) landed: Vec<LandedItem>,
    /// Any failure was `ACCESS_DENIED`, even past the capped report list.
    pub(crate) denied: bool,
}

#[derive(Clone, Default)]
pub struct OpState {
    jobs: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    /// Op ids whose ordinary run reported `ACCESS_DENIED`, with a fingerprint of the
    /// exact request. `op_retry_elevated` may only start from one of these; the entry is
    /// consumed on use so a replay cannot elevate a second time.
    denied: Arc<Mutex<HashMap<String, Denied>>>,
}

#[derive(Clone)]
struct Denied {
    request: Request,
    at: Instant,
}

impl OpState {
    fn begin(&self, op_id: &str) -> Arc<AtomicBool> {
        let token = Arc::new(AtomicBool::new(false));
        // A reused id must not orphan the running job's token: the old operation would
        // become uncancellable and keep writing after the UI forgot about it.
        if let Some(previous) = self
            .jobs
            .lock()
            .unwrap()
            .insert(op_id.to_string(), token.clone())
        {
            previous.store(true, Ordering::Relaxed);
        }
        token
    }

    pub(crate) fn begin_external(&self, op_id: &str) -> Arc<AtomicBool> {
        self.begin(op_id)
    }

    pub(crate) fn finish_external(&self, op_id: &str, token: &Arc<AtomicBool>) {
        self.finish(op_id, token);
    }

    /// Removes `op_id` only when it still belongs to `token`. A reused id installs a new
    /// token; the older thread must not delete (and hide) the newer job.
    fn finish(&self, op_id: &str, token: &Arc<AtomicBool>) {
        let mut jobs = self.jobs.lock().unwrap();
        if jobs.get(op_id).is_some_and(|current| Arc::ptr_eq(current, token)) {
            jobs.remove(op_id);
        }
    }

    pub fn cancel(&self, op_id: &str) {
        if let Some(token) = self.jobs.lock().unwrap().get(op_id) {
            token.store(true, Ordering::Relaxed);
        }
    }

    /// Ids of operations that are still running, for the shutdown confirmation.
    pub fn active(&self) -> Vec<String> {
        self.jobs.lock().unwrap().keys().cloned().collect()
    }

    pub fn cancel_all(&self) {
        for token in self.jobs.lock().unwrap().values() {
            token.store(true, Ordering::Relaxed);
        }
    }

    /// Waits until no operation is registered, up to `timeout`. The shell can take a
    /// moment to wind down an aborted `IFileOperation`, so exiting must not race it.
    pub fn wait_idle(&self, timeout: std::time::Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.jobs.lock().unwrap().is_empty() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        self.jobs.lock().unwrap().is_empty()
    }

    pub(crate) fn record_denied(&self, request: &Request) {
        let mut denied = self.denied.lock().unwrap();
        denied.retain(|_, entry| entry.at.elapsed() < std::time::Duration::from_secs(600));
        denied.insert(
            request.op_id.clone(),
            Denied {
                request: request.clone(),
                at: Instant::now(),
            },
        );
    }

    /// True exactly once for a retry of a recorded `ACCESS_DENIED` run. The retry may
    /// carry a subset of the original sources (the items that actually failed) as long as
    /// every other argument is identical.
    pub fn take_denied(&self, request: &Request) -> bool {
        let mut denied = self.denied.lock().unwrap();
        let Some(entry) = denied.get(&request.op_id) else {
            return false;
        };
        if entry.at.elapsed() >= std::time::Duration::from_secs(600) {
            denied.remove(&request.op_id);
            return false;
        }
        let original = &entry.request;
        let same_arguments = original.kind == request.kind
            && original.target_dir == request.target_dir
            && original.new_name == request.new_name
            && original.new_names == request.new_names
            && original.permanent == request.permanent
            && original.overwrite == request.overwrite;
        let subset = !request.sources.is_empty()
            && request.sources.len() <= original.sources.len()
            && request
                .sources
                .iter()
                .all(|source| original.sources.contains(source));
        if !same_arguments || !subset {
            return false;
        }
        denied.remove(&request.op_id);
        true
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Copy,
    Move,
    Delete,
    Rename,
    NewDir,
}

impl Kind {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "copy" => Some(Self::Copy),
            "move" => Some(Self::Move),
            "delete" => Some(Self::Delete),
            "rename" => Some(Self::Rename),
            "newdir" => Some(Self::NewDir),
            _ => None,
        }
    }
}

/// Result written by the elevated helper process and read back by the GUI.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ElevatedReport {
    pub aborted: bool,
    pub failed: Vec<OpFailure>,
    pub failed_total: u32,
    /// Source indices that completed; used to undo an aborted retry.
    #[serde(default)]
    pub succeeded: Vec<u32>,
    /// Indices of failed sources, uncapped; see [`OpProgress::failed_indices`].
    #[serde(default)]
    pub failed_indices: Vec<u32>,
    /// Actual destination paths reported by the shell; see [`LandedItem`].
    #[serde(default)]
    pub landed: Vec<LandedItem>,
    /// The `landed` list was capped to fit the pipe, so items in `succeeded` may have no
    /// entry. Their requested name is not safe to use for undo.
    #[serde(default)]
    pub landed_truncated: bool,
    pub error: Option<ApiError>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub op_id: String,
    pub kind: String,
    pub sources: Vec<String>,
    pub target_dir: Option<String>,
    pub new_name: Option<String>,
    /// One new name per source for a batch rename; mutually exclusive with `new_name`.
    pub new_names: Option<Vec<String>>,
    pub permanent: bool,
    /// Sync previews answer the shell's replace prompt up front; ordinary copies keep it.
    #[serde(default)]
    pub overwrite: bool,
    /// Sources the user explicitly confirmed for the `protect_confirm` tier. The guard
    /// re-checks them, so a stale or edited list cannot authorise a different path.
    #[serde(default)]
    pub confirmed_protected: Vec<String>,
}

/// Checks a request before it touches the shell; used by both the GUI path and the
/// elevated helper, which receives the request as JSON from another process.
fn validate(
    request: &Request,
    kind: Kind,
    protection: &protect::Protection,
) -> Result<(), ApiError> {
    if request.sources.is_empty() {
        return Err(ApiError::new("EMPTY", "no source paths"));
    }
    if matches!(kind, Kind::Delete | Kind::Move | Kind::Rename) {
        protect::check(
            &request.sources,
            protection,
            &request.confirmed_protected,
        )?;
    }
    if kind == Kind::Rename {
        match &request.new_names {
            Some(names) if names.len() != request.sources.len() => {
                return Err(ApiError::new(
                    "BAD_ARGS",
                    "batch rename needs one new name per source",
                ));
            }
            Some(names) => {
                for name in names {
                    validate_entry_name(name)?;
                }
            }
            None => {
                let name = request
                    .new_name
                    .as_deref()
                    .ok_or_else(|| ApiError::new("BAD_ARGS", "rename needs newName"))?;
                validate_entry_name(name)?;
            }
        }
    }
    // Copy/move `newName` is a single segment too. Without this the shell received
    // `..\..\Windows\evil.dll` (or an absolute spelling) as a name and resolved it as a
    // path, escaping `targetDir`.
    if matches!(kind, Kind::NewDir | Kind::Copy | Kind::Move) {
        if let Some(name) = request.new_name.as_deref() {
            validate_entry_name(name)?;
        }
    }
    if matches!(kind, Kind::Copy | Kind::Move | Kind::NewDir) && request.target_dir.is_none() {
        return Err(ApiError::new("BAD_ARGS", "operation needs targetDir"));
    }
    Ok(())
}

/// Rename targets and new folder names are single segments, never paths. Rejecting the
/// separators here covers both the shell path and the WebDAV `MOVE` destination.
fn validate_entry_name(name: &str) -> Result<(), ApiError> {
    let invalid = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', '\0'])
        || name.chars().any(char::is_control);
    if invalid {
        return Err(ApiError::new("INVALID_NAME", name.to_string()));
    }
    Ok(())
}

fn involves_webdav(request: &Request) -> bool {
    request.sources.iter().any(|path| crate::webdav::is_webdav(path))
        || request
            .target_dir
            .as_deref()
            .is_some_and(crate::webdav::is_webdav)
}

/// Runs an operation to completion on the calling thread. This is the body of the
/// elevated helper: it has no `AppHandle`, no window and no Channel back to a webview,
/// so progress is discarded and only the final result matters.
pub fn run_headless(
    request: Request,
    protection: protect::Protection,
    cancel: Arc<AtomicBool>,
) -> ElevatedReport {
    let report = |error: Option<ApiError>| ElevatedReport {
        aborted: false,
        failed: Vec::new(),
        failed_total: 0,
        succeeded: Vec::new(),
        failed_indices: Vec::new(),
        landed: Vec::new(),
        landed_truncated: false,
        error,
    };
    let kind = match Kind::parse(&request.kind) {
        Some(kind) => kind,
        None => {
            return report(Some(ApiError::new(
                "BAD_KIND",
                format!("unknown operation {}", request.kind),
            )))
        }
    };
    if let Err(err) = validate(&request, kind, &protection) {
        return report(Some(err));
    }
    if involves_webdav(&request) {
        return report(Some(ApiError::new(
            "BAD_ARGS",
            "WebDAV operations must run in the GUI process",
        )));
    }
    let outcome = run_guarded(&request.kind, || {
        let sink = Channel::new(|_| Ok(()));
        run(
            &request,
            kind,
            None,
            cancel,
            sink,
            &crate::settings::Settings::default(),
        )
    });
    // The failure list is capped, so a truncated report needs the index list to tell which
    // items actually landed when the caller records an undo entry.
    let truncated = outcome.failed_total as usize > outcome.failed.len();
    let (landed, landed_truncated) = cap_landed(outcome.landed);
    ElevatedReport {
        aborted: outcome.aborted,
        failed: outcome.failed,
        failed_total: outcome.failed_total,
        succeeded: if outcome.aborted || truncated {
            outcome.succeeded
        } else {
            Vec::new()
        },
        failed_indices: outcome.failed_indices,
        landed,
        landed_truncated,
        error: outcome.error,
    }
}

/// The elevated report travels over a 16 MiB named pipe. Full destination paths for a
/// huge batch would exceed it and turn a completed operation into a failed retry, so keep
/// a bounded head; undo falls back to the requested name for the rest.
const MAX_ELEVATED_LANDED_BYTES: usize = 4 * 1024 * 1024;

/// Returns the kept items and whether any were dropped.
fn cap_landed(items: Vec<LandedItem>) -> (Vec<LandedItem>, bool) {
    let mut out = Vec::new();
    let mut total = 0usize;
    for item in items {
        // Rough per-entry JSON overhead: index, quotes, comma.
        let cost = item.path.len() + 16;
        if total.saturating_add(cost) > MAX_ELEVATED_LANDED_BYTES {
            return (out, true);
        }
        total += cost;
        out.push(item);
    }
    (out, false)
}

/// Starts an operation on a fresh STA thread and returns immediately.
pub fn start(
    state: &OpState,
    request: Request,
    protection: &protect::Protection,
    hwnd: Option<isize>,
    progress: Channel<OpProgress>,
    settings: Arc<crate::settings::Settings>,
) -> Result<(), ApiError> {
    let kind = Kind::parse(&request.kind)
        .ok_or_else(|| ApiError::new("BAD_KIND", format!("unknown operation {}", request.kind)))?;
    validate(&request, kind, protection)?;
    let token = state.begin(&request.op_id);
    let finish_token = token.clone();
    let err_token = finish_token.clone();
    let op_id = request.op_id.clone();
    let err_op_id = op_id.clone();
    let err_state = state.clone();
    let state = state.clone();

    let spawned = std::thread::Builder::new()
        .name(format!("flowdir-op-{op_id}"))
        .spawn(move || {
            let total = request.sources.len() as u32;
            let outcome = run_guarded(&request.kind, || {
                run(
                    &request,
                    kind,
                    hwnd,
                    token,
                    progress.clone(),
                    &settings,
                )
            });
            match (&outcome.error, outcome.aborted, outcome.failed_total) {
                (Some(err), _, _) => log::error!(
                    target: "flowdir::ops",
                    "op {} failed: {} {}",
                    request.kind,
                    err.code,
                    err.detail
                ),
                (None, true, _) => log::warn!(
                    target: "flowdir::ops",
                    "op {} canceled after {} items",
                    request.kind,
                    total
                ),
                (None, false, failed) if failed > 0 => log::warn!(
                    target: "flowdir::ops",
                    "op {} partial: {failed} of {total} items failed",
                    request.kind
                ),
                (None, false, _) => log::info!(
                    target: "flowdir::ops",
                    "op {} done: {total} items",
                    request.kind
                ),
            }
            let denied = outcome_denied(&outcome);
            let truncated = outcome.failed_total as usize > outcome.failed.len();
            let succeeded = if outcome.aborted || truncated {
                outcome.succeeded
            } else {
                Vec::new()
            };
            let _ = progress.send(OpProgress {
                op_id: op_id.clone(),
                phase: "done".into(),
                current: total,
                total,
                name: None,
                work: 0,
                work_total: 0,
                failed_count: outcome.failed_total,
                failed: outcome.failed,
                // The index list is needed by an aborted batch (to undo the part that
                // landed) and by a batch whose failure list was capped: the paths alone
                // then cannot tell which items succeeded.
                succeeded,
                // Uncapped: the elevated retry replays exactly this subset.
                failed_indices: outcome.failed_indices,
                landed: outcome.landed,
                done: true,
                aborted: outcome.aborted,
                error: outcome.error,
            });
            if denied {
                state.record_denied(&request);
            }
            state.finish(&op_id, &finish_token);
        });
    // Without this the token would stay in `jobs` forever: a ghost operation that
    // keeps `wait_idle` timing out and never sends the frontend a `done` progress.
    if spawned.is_err() {
        err_state.finish(&err_op_id, &err_token);
    }
    spawned.map(|_| ()).map_err(|e| ApiError::new("THREAD", e.to_string()))
}

fn outcome_denied(outcome: &Outcome) -> bool {
    // `failed` stops at the report cap, so the flag set at record time is what counts.
    outcome.denied
        || outcome
            .error
            .as_ref()
            .is_some_and(|error| error.code == "ACCESS_DENIED")
}

/// Runs the operation body and turns a panic into a normal failure: `finish` still runs,
/// so the job cannot stay in the map (and in the taskbar progress) forever.
fn run_guarded<F: FnOnce() -> Outcome>(kind: &str, work: F) -> Outcome {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)) {
        Ok(outcome) => outcome,
        Err(payload) => {
            let detail = payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "operation thread panicked".into());
            log::error!(target: "flowdir::ops", "op {kind} panicked: {detail}");
            Outcome {
                aborted: false,
                error: Some(ApiError::new("PANIC", detail)),
                failed: Vec::new(),
                failed_total: 0,
                succeeded: Vec::new(),
                failed_indices: Vec::new(),
                landed: Vec::new(),
                denied: false,
            }
        }
    }
}

fn run(
    request: &Request,
    kind: Kind,
    hwnd: Option<isize>,
    cancel: Arc<AtomicBool>,
    progress: Channel<OpProgress>,
    settings: &crate::settings::Settings,
) -> Outcome {
    // Remote paths never reach `IFileOperation`: it cannot address them, and the WebDAV
    // runner owns the transfer, progress and cancellation for both sides of a mixed batch.
    if involves_webdav(request) {
        return crate::webdav_ops::run(request, kind, cancel, progress, settings);
    }
    unsafe {
        if let Err(e) = CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok() {
            return Outcome {
                aborted: false,
                error: Some(ApiError::new("COM", e.to_string())),
                failed: Vec::new(),
                failed_total: 0,
                succeeded: Vec::new(),
                failed_indices: Vec::new(),
                landed: Vec::new(),
                denied: false,
            };
        }
        let outcome = perform(request, kind, hwnd, cancel, progress);
        CoUninitialize();
        outcome
    }
}

/// Prefixes long paths with the verbatim marker before they reach the shell. The shell
/// rejects paths past MAX_PATH otherwise, and the manifest's `longPathAware` only covers
/// the API surface, not every internal shell parser.
fn shell_path(path: &str) -> String {
    // Ordinary paths keep their usual form: the marker also disables `..`/`.` cleanup
    // and trailing-dot trimming inside the shell, which would change their meaning.
    // MAX_PATH is counted in UTF-16 units, not UTF-8 bytes. Counting bytes prefixed
    // long-before-needed paths, and the verbatim marker disables `..` collapsing and
    // trailing-dot trimming, changing what the path means.
    if path.encode_utf16().count() <= 240 || path.starts_with(r"\\?\") {
        return path.to_string();
    }
    let bytes = path.as_bytes();
    if let Some(rest) = path.strip_prefix(r"\\") {
        return format!(r"\\?\UNC\{rest}");
    }
    // Drive-letter absolute path, e.g. `C:\dir\file`. A drive-relative `C:file` is left
    // alone: the marker would make it absolute and point at a different location.
    if bytes.len() > 2 && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/') {
        return format!(r"\\?\{path}");
    }
    path.to_string()
}

fn shell_item(path: &str) -> Result<IShellItem, ApiError> {
    let wide = HSTRING::from(shell_path(path));
    unsafe {
        SHCreateItemFromParsingName::<_, Option<&windows::Win32::System::Com::IBindCtx>, IShellItem>(
            &wide,
            None,
        )
            .map_err(|e| ApiError::from_windows(path, e.code().0))
    }
}

/// Filesystem path of a shell item, for the failure report and the delete caption.
fn item_path(item: &IShellItem) -> Option<String> {
    unsafe {
        let wide = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let text = wide.to_string().ok();
        CoTaskMemFree(Some(wide.0 as *const core::ffi::c_void));
        text
    }
}

/// Last path segment, used as the progress caption when the shell passes no name.
fn file_name(path: &str) -> Option<String> {
    path.rsplit(['\\', '/'])
        .find(|part| !part.is_empty())
        .map(str::to_string)
}

fn perform(
    request: &Request,
    kind: Kind,
    hwnd: Option<isize>,
    cancel: Arc<AtomicBool>,
    progress: Channel<OpProgress>,
) -> Outcome {
    let total = request.sources.len() as u32;
    let failures = Arc::new(FailureLog::default());
    let succeeded: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
    let failed_indices: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
    let landed: Arc<Mutex<Vec<LandedItem>>> = Arc::new(Mutex::new(Vec::new()));
    let source_index: Arc<HashMap<String, u32>> = Arc::new(
        request
            .sources
            .iter()
            .enumerate()
            .map(|(index, path)| (path.to_lowercase(), index as u32))
            .collect(),
    );
    let sink = Sink {
        op_id: request.op_id.clone(),
        phase: AtomicU8::new(0),
        current: AtomicU32::new(0),
        total: AtomicU32::new(total),
        work: AtomicU64::new(0),
        work_total: AtomicU64::new(0),
        last: Mutex::new(
            Instant::now() - std::time::Duration::from_millis(PROGRESS_INTERVAL_MS as u64),
        ),
        current_path: Mutex::new(String::new()),
        failures: failures.clone(),
        succeeded: succeeded.clone(),
        failed_indices: failed_indices.clone(),
        landed: landed.clone(),
        source_index,
        canonical_index: Arc::new(OnceLock::new()),
        cancel,
        progress,
    };

    unsafe {
        let failure = |error: ApiError| Outcome {
            aborted: false,
            error: Some(error),
            failed: failures.take(),
            failed_total: failures.count(),
            succeeded: succeeded.lock().unwrap().clone(),
            failed_indices: failed_indices.lock().unwrap().clone(),
            landed: landed.lock().unwrap().clone(),
            denied: failures.denied(),
        };
        let operation: IFileOperation = match CoCreateInstance(&FileOperation, None, CLSCTX_ALL) {
            Ok(operation) => operation,
            Err(e) => return failure(ApiError::new("SHELL_FAILED", e.to_string())),
        };
        if let Some(raw) = hwnd {
            let _ = operation.SetOwnerWindow(HWND(raw as *mut core::ffi::c_void));
        }
        // Failures are reported in-app (with retryable entries), so the shell must not put a
        // modal in front of every item that fails. Nothing here suppresses the shell's own
        // replace/overwrite prompts: copies and moves keep asking through IFileOperation.
        // Deletes are confirmed in the app first, so the permanent-delete prompt is redundant;
        // a Recycle Bin delete keeps FOF_WANTNUKEWARNING in case the shell has to nuke instead.
        let mut flags = FOF_SILENT | FOF_NOERRORUI;
        if !request.permanent {
            // `FOF_WANTNUKEWARNING` overrides `FOF_NOCONFIRMATION`, so it must not be set
            // for a Shift+Delete the app already confirmed.
            flags |= FOF_ALLOWUNDO | FOF_WANTNUKEWARNING;
        }
        // A sync preview already showed every overwrite; asking per file would defeat it.
        if request.overwrite {
            flags |= FOF_NOCONFIRMATION;
        }
        if kind == Kind::Delete {
            if request.permanent {
                flags |= FOF_NOCONFIRMATION;
            } else {
                flags |= FOFX_RECYCLEONDELETE;
            }
        }
        if let Err(e) = operation.SetOperationFlags(flags) {
            return failure(ApiError::new("SHELL_FAILED", e.to_string()));
        }

        let sink_iface: IFileOperationProgressSink = sink.into();
        let cookie = match operation.Advise(&sink_iface) {
            Ok(cookie) => cookie,
            Err(e) => return failure(ApiError::new("SHELL_FAILED", e.to_string())),
        };

        let queued = queue(&operation, request, kind, &failures, &failed_indices);
        let result = match queued {
            // Every source already failed to resolve; the shell has nothing to perform and
            // would report an unrelated error of its own.
            Ok(0) => Ok(()),
            Ok(_) => operation
                .PerformOperations()
                .map_err(|e| ApiError::from_windows("", e.code().0)),
            Err(err) => Err(err),
        };
        let aborted = operation
            .GetAnyOperationsAborted()
            .map(|value| value.as_bool())
            .unwrap_or(false);
        let _ = operation.Unadvise(cookie);

        let failed_total = failures.count();
        let error = match result {
            Ok(()) => None,
            // A canceled operation is not an error: the items it never reached are not failures.
            Err(_) if aborted => None,
            // The shell's error is a consequence of the item failures already recorded; the
            // list is what the user can act on, so the batch error adds nothing.
            Err(_) if failed_total > 0 => None,
            Err(err) => Some(err),
        };
        let completed: Vec<u32> = succeeded.lock().unwrap().clone();
        let reported_failed: Vec<u32> = failed_indices.lock().unwrap().clone();
        let landed_items = landed.lock().unwrap().clone();
        Outcome {
            aborted,
            error,
            failed: failures.take(),
            failed_total,
            succeeded: completed,
            failed_indices: reported_failed,
            landed: landed_items,
            denied: failures.denied(),
        }
    }
}

fn queue(
    operation: &IFileOperation,
    request: &Request,
    kind: Kind,
    failures: &FailureLog,
    failed_indices: &Mutex<Vec<u32>>,
) -> Result<usize, ApiError> {
    unsafe {
        let failure = |path: &str, error: ApiError| failures.push(path, error);
        if kind == Kind::NewDir {
            let dir = request
                .target_dir
                .as_deref()
                .ok_or_else(|| ApiError::new("BAD_ARGS", "newdir needs targetDir"))?;
            let name_text = request
                .new_name
                .as_deref()
                .ok_or_else(|| ApiError::new("BAD_ARGS", "newdir needs newName"))?;
            let folder = shell_item(dir)?;
            let name = HSTRING::from(name_text);
            return match operation.NewItem(
                &folder,
                0x10,
                &name,
                PCWSTR::null(),
                None::<&IFileOperationProgressSink>,
            ) {
                Ok(()) => Ok(1),
                Err(e) => {
                    failure(dir, ApiError::from_windows("", e.code().0));
                    Ok(0)
                }
            };
        }

        let target = match request.target_dir.as_deref() {
            Some(dir) => Some(shell_item(dir)?),
            None => None,
        };
        let new_name = request.new_name.as_deref().map(HSTRING::from);
        let mut queued_count = 0;

        for (index, path) in request.sources.iter().enumerate() {
            let item = match shell_item(path) {
                Ok(item) => item,
                // One unresolvable path (deleted meanwhile, unparsable) must not abort the
                // whole batch; report it and keep queueing the rest.
                Err(err) => {
                    failed_indices.lock().unwrap().push(index as u32);
                    failure(
                        path,
                        ApiError::new(&err.code, err.detail),
                    );
                    continue;
                }
            };
            let queued = match kind {
                Kind::Copy => match &new_name {
                    Some(name) => operation.CopyItem(
                        &item,
                        target.as_ref(),
                        name,
                        None::<&IFileOperationProgressSink>,
                    ),
                    None => operation.CopyItem(
                        &item,
                        target.as_ref(),
                        PCWSTR::null(),
                        None::<&IFileOperationProgressSink>,
                    ),
                },
                Kind::Move => match &new_name {
                    Some(name) => operation.MoveItem(
                        &item,
                        target.as_ref(),
                        name,
                        None::<&IFileOperationProgressSink>,
                    ),
                    None => operation.MoveItem(
                        &item,
                        target.as_ref(),
                        PCWSTR::null(),
                        None::<&IFileOperationProgressSink>,
                    ),
                },
                Kind::Delete => operation.DeleteItem(&item, None::<&IFileOperationProgressSink>),
                Kind::Rename => {
                    let name = request
                        .new_names
                        .as_ref()
                        .and_then(|names| names.get(index))
                        .map(HSTRING::from)
                        .or_else(|| request.new_name.as_ref().map(HSTRING::from))
                        .ok_or_else(|| ApiError::new("BAD_ARGS", "rename needs newName"))?;
                    operation.RenameItem(
                        &item,
                        &name,
                        None::<&IFileOperationProgressSink>,
                    )
                }
                Kind::NewDir => unreachable!(),
            };
            if let Err(e) = queued {
                failed_indices.lock().unwrap().push(index as u32);
                failure(path, ApiError::from_windows(path, e.code().0));
            } else {
                queued_count += 1;
            }
        }
        Ok(queued_count)
    }
}

/// Progress sink: throttles updates to ~12/s, but always reports item boundaries.
#[implement(IFileOperationProgressSink)]
struct Sink {
    op_id: String,
    phase: AtomicU8,
    current: AtomicU32,
    total: AtomicU32,
    work: AtomicU64,
    work_total: AtomicU64,
    last: Mutex<Instant>,
    /// Path of the item currently being processed, so a Post callback can name its failure.
    current_path: Mutex<String>,
    failures: Arc<FailureLog>,
    /// Source indices of completed items; see [`Outcome::succeeded`].
    succeeded: Arc<Mutex<Vec<u32>>>,
    /// Source indices of failed items, uncapped; see [`Outcome::failed_indices`].
    failed_indices: Arc<Mutex<Vec<u32>>>,
    /// Actual destination of each completed item, reported by the `Post*` callbacks.
    landed: Arc<Mutex<Vec<LandedItem>>>,
    /// Lowercased source path → index, so a Post callback can map its item back.
    source_index: Arc<HashMap<String, u32>>,
    /// Canonical source key → index, built on the first lookup miss. The shell reports
    /// its own spelling (8.3 short names, different case), which the raw map misses.
    canonical_index: Arc<OnceLock<HashMap<String, u32>>>,
    cancel: Arc<AtomicBool>,
    progress: Channel<OpProgress>,
}

/// Cancellation surfaces as `E_ABORT` or one of the copy engine's cancel codes. These are
/// outcomes the user asked for, not item failures.
fn is_cancelled(hr: HRESULT) -> bool {
    hr == E_ABORT
        || hr.0 == 0x8027_0000u32 as i32 // COPYENGINE_E_USER_CANCELLED
        || hr.0 == 0x8027_0001u32 as i32 // COPYENGINE_E_CANCELLED
}

impl Sink {
    /// Source index of the item the shell is reporting.
    fn source_of(&self, path: &str) -> Option<u32> {
        let raw = path.to_lowercase();
        if let Some(index) = self.source_index.get(&raw) {
            return Some(*index);
        }
        let canonical = self.canonical_index.get_or_init(|| {
            self.source_index
                .iter()
                .map(|(path, index)| (crate::pathkey::path_key(path), *index))
                .collect()
        });
        canonical.get(&crate::pathkey::path_key(&raw)).copied()
    }

    fn emit(&self, name: Option<String>, force: bool) {
        let mut last = self.last.lock().unwrap();
        if !force && last.elapsed().as_millis() < PROGRESS_INTERVAL_MS {
            return;
        }
        *last = Instant::now();
        let _ = self.progress.send(OpProgress {
            op_id: self.op_id.clone(),
            phase: PHASES[self.phase.load(Ordering::Relaxed) as usize].to_string(),
            current: self.current.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
            name,
            work: self.work.load(Ordering::Relaxed),
            work_total: self.work_total.load(Ordering::Relaxed),
            failed_count: self.failures.count(),
            failed: Vec::new(),
            succeeded: Vec::new(),
            failed_indices: Vec::new(),
            landed: Vec::new(),
            done: false,
            aborted: false,
            error: None,
        });
    }

    fn check_cancel(&self) -> WinResult<()> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(windows::core::Error::from_hresult(E_ABORT));
        }
        Ok(())
    }

    fn enter_item(&self, phase: &'static str, name: &PCWSTR, item: Option<&IShellItem>) -> WinResult<()> {
        self.check_cancel()?;
        self.phase.store(
            PHASES.iter().position(|p| *p == phase).unwrap_or(0) as u8,
            Ordering::Relaxed,
        );
        self.current.fetch_add(1, Ordering::Relaxed);
        // Replace, never leave the previous item's path in place: `Post*Item` would then
        // attribute this item's success/failure and destination to the wrong source.
        let captured = item.and_then(item_path).unwrap_or_default();
        *self.current_path.lock().unwrap() = captured;
        let label = if name.is_null() {
            // Deletes arrive without a name; the path we captured above supplies the caption.
            file_name(&self.current_path.lock().unwrap())
        } else {
            Some(unsafe { name.to_string() }.unwrap_or_default())
        };
        self.emit(label, true);
        Ok(())
    }

    /// A failing item is not an abort: record it, keep the queue going, and let the final
    /// message carry the list so the UI can say "X done, Y failed" instead of "done".
    fn post_item(&self, hr: HRESULT) -> WinResult<()> {
        if hr.is_ok() {
            // Record the success before checking the cancel flag: an abort that arrives
            // while this callback runs must not hide an item that already landed.
            let path = self.current_path.lock().unwrap();
            if let Some(index) = self.source_of(&path) {
                self.succeeded.lock().unwrap().push(index);
            }
        } else if !is_cancelled(hr) {
            // A cancel only explains `E_ABORT` or the copy engine's own cancel codes
            // (`COPYENGINE_E_USER_CANCELLED` / `COPYENGINE_E_CANCELLED`). Any other
            // failure after the flag was set still happened, and dropping it would hide an
            // item from the failure list, the elevated retry and the undo subset.
            let path = self.current_path.lock().unwrap();
            if let Some(index) = self.source_of(&path) {
                self.failed_indices.lock().unwrap().push(index);
            }
            self.failures.record(&path, hr);
            self.emit(None, true);
        }
        self.check_cancel()
    }

    /// Records the item's real destination. `Post*Item` receives the created item, which
    /// differs from the requested name when the user resolves a conflict with "keep
    /// both" (`name (1).ext`); undoing the requested name would target a different file.
    fn post_item_new(&self, hr: HRESULT, new: Option<&IShellItem>) -> WinResult<()> {
        if hr.is_ok() {
            if let Some(path) = new.and_then(item_path) {
                let index = {
                    let source = self.current_path.lock().unwrap();
                    self.source_of(&source)
                };
                if let Some(index) = index {
                    self.landed.lock().unwrap().push(LandedItem { index, path });
                }
            }
        }
        self.post_item(hr)
    }
}

impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn StartOperations(&self) -> WinResult<()> {
        self.emit(None, true);
        Ok(())
    }

    fn FinishOperations(&self, hr: HRESULT) -> WinResult<()> {
        // `PerformOperations` can return S_OK with a failing final HRESULT here. A user
        // cancel also arrives as a failure; recording it fabricated an unnamed failure
        // next to `aborted: true`.
        if hr.is_err() && !is_cancelled(hr) {
            self.failures.record("", hr);
        }
        Ok(())
    }

    fn PreRenameItem(
        &self,
        _flags: u32,
        item: windows::core::Ref<'_, IShellItem>,
        name: &PCWSTR,
    ) -> WinResult<()> {
        self.enter_item("rename", name, item.as_ref())
    }

    fn PostRenameItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
        _name: &PCWSTR,
        hr: HRESULT,
        new: windows::core::Ref<'_, IShellItem>,
    ) -> WinResult<()> {
        self.post_item_new(hr, new.as_ref())
    }

    fn PreMoveItem(
        &self,
        _flags: u32,
        item: windows::core::Ref<'_, IShellItem>,
        _dir: windows::core::Ref<'_, IShellItem>,
        name: &PCWSTR,
    ) -> WinResult<()> {
        self.enter_item("move", name, item.as_ref())
    }

    fn PostMoveItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
        _dir: windows::core::Ref<'_, IShellItem>,
        _name: &PCWSTR,
        hr: HRESULT,
        new: windows::core::Ref<'_, IShellItem>,
    ) -> WinResult<()> {
        self.post_item_new(hr, new.as_ref())
    }

    fn PreCopyItem(
        &self,
        _flags: u32,
        item: windows::core::Ref<'_, IShellItem>,
        _dir: windows::core::Ref<'_, IShellItem>,
        name: &PCWSTR,
    ) -> WinResult<()> {
        self.enter_item("copy", name, item.as_ref())
    }

    fn PostCopyItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
        _dir: windows::core::Ref<'_, IShellItem>,
        _name: &PCWSTR,
        hr: HRESULT,
        new: windows::core::Ref<'_, IShellItem>,
    ) -> WinResult<()> {
        self.post_item_new(hr, new.as_ref())
    }

    fn PreDeleteItem(
        &self,
        _flags: u32,
        item: windows::core::Ref<'_, IShellItem>,
    ) -> WinResult<()> {
        self.enter_item("delete", &PCWSTR::null(), item.as_ref())
    }

    fn PostDeleteItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
        hr: HRESULT,
        _new: windows::core::Ref<'_, IShellItem>,
    ) -> WinResult<()> {
        self.post_item(hr)
    }

    fn PreNewItem(
        &self,
        _flags: u32,
        dir: windows::core::Ref<'_, IShellItem>,
        name: &PCWSTR,
    ) -> WinResult<()> {
        let label = unsafe { name.to_string() }.unwrap_or_default();
        if let Some(base) = dir.as_ref().and_then(item_path) {
            *self.current_path.lock().unwrap() = format!("{base}\\{label}");
        }
        self.enter_item("newdir", name, None)
    }

    fn PostNewItem(
        &self,
        _flags: u32,
        _dir: windows::core::Ref<'_, IShellItem>,
        _name: &PCWSTR,
        _template: &PCWSTR,
        _attrs: u32,
        hr: HRESULT,
        _new: windows::core::Ref<'_, IShellItem>,
    ) -> WinResult<()> {
        self.post_item(hr)
    }

    fn UpdateProgress(&self, work_total: u32, work_so_far: u32) -> WinResult<()> {
        self.check_cancel()?;
        self.work_total.store(work_total as u64, Ordering::Relaxed);
        self.work.store(work_so_far as u64, Ordering::Relaxed);
        self.emit(None, false);
        Ok(())
    }

    fn ResetTimer(&self) -> WinResult<()> {
        Ok(())
    }

    fn PauseTimer(&self) -> WinResult<()> {
        Ok(())
    }

    fn ResumeTimer(&self) -> WinResult<()> {
        Ok(())
    }
}


/// Test hook: deletes one path to the Recycle Bin through the real shell pipeline and
/// returns the number of failed items, so the restore path can be exercised end to end.
#[cfg(test)]
pub(crate) fn recycle_for_test(path: &str) -> u32 {
    let request = Request {
        op_id: "test-recycle".into(),
        kind: "delete".into(),
        sources: vec![path.to_string()],
        target_dir: None,
        new_name: None,
        new_names: None,
        permanent: false,
        overwrite: false,
        confirmed_protected: Vec::new(),
    };
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let outcome = perform(
        &request,
        Kind::Delete,
        None,
        Arc::new(AtomicBool::new(false)),
        Channel::new(|_| Ok(())),
    );
    unsafe {
        CoUninitialize();
    }
    outcome.error.map(|_| 1).unwrap_or(outcome.failed_total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::windows::fs::OpenOptionsExt;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("flowdir-op-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn panic_becomes_a_failed_outcome() {
        let outcome = run_guarded("copy", || panic!("boom"));
        let error = outcome.error.expect("a panic must surface as an error");
        assert_eq!(error.code, "PANIC");
        assert!(error.detail.contains("boom"), "{}", error.detail);
        assert!(!outcome.aborted);
    }

    #[test]
    fn shell_path_only_rewrites_long_absolute_paths() {
        fn pad(base: &str) -> String {
            let mut path = base.to_string();
            while path.len() <= 240 {
                path.push('x');
            }
            path
        }

        // Short paths keep their exact spelling: the verbatim marker would also disable
        // `.`/`..` collapsing and trailing-dot trimming inside the shell.
        assert_eq!(shell_path(r"C:\short\file.txt"), r"C:\short\file.txt");
        assert_eq!(shell_path(r"relative\file.txt"), r"relative\file.txt");
        assert_eq!(
            shell_path(r"\\server\share\file.txt"),
            r"\\server\share\file.txt"
        );

        let drive = pad(r"C:\dir\");
        assert_eq!(shell_path(&drive), format!(r"\\?\{drive}"));

        let unc = pad(r"\\server\share\");
        assert_eq!(shell_path(&unc), format!(r"\\?\UNC\{}", &unc[2..]));

        // Already verbatim, and drive-relative paths, are left alone.
        assert_eq!(shell_path(&pad(r"\\?\C:\")), pad(r"\\?\C:\"));
        assert_eq!(shell_path(&pad("C:relative")), pad("C:relative"));
    }

    #[test]
    fn elevated_retry_accepts_the_denied_subset_only_once() {
        let state = OpState::default();
        let original = Request {
            op_id: "op-denied".into(),
            kind: "copy".into(),
            sources: vec!["C:\\a.txt".into(), "C:\\b.txt".into()],
            target_dir: Some("C:\\dest".into()),
            new_name: None,
            new_names: None,
            permanent: false,
            overwrite: false,
            confirmed_protected: Vec::new(),
        };
        state.record_denied(&original);
        let subset = Request {
            sources: vec!["C:\\b.txt".into()],
            ..original.clone()
        };
        assert!(state.take_denied(&subset));
        assert!(!state.take_denied(&subset), "the record is consumed");

        // A retry aimed at another destination must not be elevated.
        state.record_denied(&original);
        let retargeted = Request {
            target_dir: Some("C:\\elsewhere".into()),
            ..original
        };
        assert!(!state.take_denied(&retargeted));
    }

    #[test]
    fn headless_rename_reports_success() {
        let dir = temp_dir("headless");
        let source = dir.join("before.txt");
        fs::write(&source, b"x").unwrap();
        let report = run_headless(
            Request {
                op_id: "headless".into(),
                kind: "rename".into(),
                sources: vec![source.to_string_lossy().into_owned()],
                target_dir: None,
                new_name: Some("after.txt".into()),
                new_names: None,
                permanent: false,
                overwrite: false,
                confirmed_protected: Vec::new(),
            },
            protect::Protection::default(),
            Arc::new(AtomicBool::new(false)),
        );
        assert!(report.error.is_none(), "{:?}", report.error);
        assert_eq!(report.failed_total, 0);
        assert!(!report.aborted);
        assert!(dir.join("after.txt").exists());
        assert!(!source.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn headless_rejects_an_unknown_kind() {
        let report = run_headless(
            Request {
                op_id: "headless".into(),
                kind: "explode".into(),
                sources: vec!["C:\\".into()],
                target_dir: None,
                new_name: None,
                new_names: None,
                permanent: false,
                overwrite: false,
                confirmed_protected: Vec::new(),
            },
            protect::Protection::default(),
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(report.error.unwrap().code, "BAD_KIND");
    }

    #[test]
    fn batch_rename_rejects_mismatched_names() {
        let channel = Channel::new(|_| Ok(()));
        let request = Request {
            op_id: "bad-batch".into(),
            kind: "rename".into(),
            // Absolute paths: relative spellings are now refused by the protection
            // pass before the name-count check.
            sources: vec![r"C:\a".into(), r"C:\b".into()],
            target_dir: None,
            new_name: None,
            new_names: Some(vec!["one".into()]),
            permanent: false,
            overwrite: false,
            confirmed_protected: Vec::new(),
        };
        let error = start(
            &OpState::default(),
            request,
            &protect::Protection::default(),
            None,
            channel,
            Arc::new(crate::settings::Settings::default()),
        )
        .unwrap_err();
        assert_eq!(error.code, "BAD_ARGS");
    }

    #[test]
    fn a_stale_finish_does_not_remove_the_reused_id() {
        let state = OpState::default();
        let first = state.begin("same");
        let second = state.begin("same");
        assert!(
            first.load(std::sync::atomic::Ordering::Relaxed),
            "replacing an id must cancel the job it belonged to"
        );
        state.finish("same", &first);
        assert_eq!(state.active(), vec!["same".to_string()]);
        state.finish("same", &second);
        assert!(state.active().is_empty());
    }

    /// The sink maps each completed item back to its source index; an aborted batch uses
    /// that list to undo the part that landed (the failure list is capped).
    #[test]
    fn completed_items_are_mapped_to_source_indices() {
        let dir = temp_dir("succeeded-indices");
        let first = dir.join("first.txt");
        let second = dir.join("second.txt");
        fs::write(&first, b"a").unwrap();
        fs::write(&second, b"b").unwrap();
        let request = Request {
            op_id: "indices".into(),
            kind: "rename".into(),
            sources: vec![
                first.to_string_lossy().into_owned(),
                second.to_string_lossy().into_owned(),
            ],
            target_dir: None,
            new_name: None,
            new_names: Some(vec!["one.txt".into(), "two.txt".into()]),
            permanent: false,
            overwrite: false,
            confirmed_protected: Vec::new(),
        };
        let outcome = run(
            &request,
            Kind::Rename,
            None,
            Arc::new(AtomicBool::new(false)),
            Channel::new(|_| Ok(())),
            &crate::settings::Settings::default(),
        );
        assert!(outcome.error.is_none(), "{outcome:?}");
        assert_eq!(outcome.failed_total, 0);
        let mut indices = outcome.succeeded;
        indices.sort_unstable();
        assert_eq!(indices, vec![0, 1]);
        let _ = fs::remove_dir_all(dir);
    }

    /// One IFileOperation renames the whole selection and reports per-item progress.
    #[test]
    fn batch_rename_renames_every_source() {
        let dir = temp_dir("batch-rename");
        let first = dir.join("first.txt");
        let second = dir.join("second.txt");
        fs::write(&first, b"a").unwrap();
        fs::write(&second, b"b").unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        let channel = Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(text) = body {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                    if value["done"] == true {
                        let _ = tx.send(value);
                    }
                }
            }
            Ok(())
        });
        let request = Request {
            op_id: "batch-rename".into(),
            kind: "rename".into(),
            sources: vec![
                first.to_string_lossy().into_owned(),
                second.to_string_lossy().into_owned(),
            ],
            target_dir: None,
            new_name: None,
            new_names: Some(vec!["one.txt".into(), "two.txt".into()]),
            permanent: false,
            overwrite: false,
            confirmed_protected: Vec::new(),
        };
        start(
            &OpState::default(),
            request,
            &protect::Protection::default(),
            None,
            channel,
            Arc::new(crate::settings::Settings::default()),
        )
        .unwrap();
        let done = rx
            .recv_timeout(std::time::Duration::from_secs(15))
            .expect("the operation must finish");
        assert_eq!(done["failedCount"], 0, "{done}");
        assert!(done["error"].is_null(), "{done}");
        assert!(dir.join("one.txt").exists());
        assert!(dir.join("two.txt").exists());
        assert!(!first.exists() && !second.exists());
        let _ = fs::remove_dir_all(dir);
    }

    fn delete_request(path: &std::path::Path) -> Request {
        Request {
            op_id: "test".into(),
            kind: "delete".into(),
            sources: vec![path.to_string_lossy().into_owned()],
            target_dir: None,
            new_name: None,
            new_names: None,
            permanent: false,
            overwrite: false,
            confirmed_protected: Vec::new(),
        }
    }

    fn copy_request(path: &std::path::Path, target: &std::path::Path, name: &str) -> Request {
        Request {
            op_id: "test".into(),
            kind: "copy".into(),
            sources: vec![path.to_string_lossy().into_owned()],
            target_dir: Some(target.to_string_lossy().into_owned()),
            new_name: Some(name.into()),
            new_names: None,
            permanent: false,
            overwrite: false,
            confirmed_protected: Vec::new(),
        }
    }

    /// The shell returns S_OK from `PerformOperations` even when the single item failed;
    /// the sink must turn the per-item HRESULT into a reported failure.
    #[test]
    fn locked_item_is_reported_as_failure() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let dir = temp_dir("locked");
        let path = dir.join("locked.txt");
        fs::write(&path, b"locked").unwrap();
        // Share mode 0: the shell's copy cannot even open the source for reading.
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();

        let outcome = perform(
            &copy_request(&path, &dir, "copy.txt"),
            Kind::Copy,
            None,
            Arc::new(AtomicBool::new(false)),
            Channel::new(|_| Ok(())),
        );

        assert!(outcome.error.is_none(), "operation-level error");
        assert_eq!(outcome.failed_total, 1, "detail: {outcome:?}");
        assert_eq!(outcome.failed.len(), 1);
        assert!(outcome.failed[0].path.ends_with("locked.txt"));
        assert_eq!(
            outcome.failed[0].code, "SHARE_UNAVAILABLE",
            "detail: {}",
            outcome.failed[0].detail
        );
        assert!(!dir.join("copy.txt").exists(), "the copy must not exist");

        drop(held);
        let _ = fs::remove_dir_all(&dir);
        unsafe {
            CoUninitialize();
        }
    }

    /// A path that no longer resolves must be reported, either from the queue or as the
    /// batch result, and must never turn into a silent success.
    #[test]
    fn missing_source_is_reported() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let dir = temp_dir("missing");
        let path = dir.join("gone.txt");

        let outcome = perform(
            &delete_request(&path),
            Kind::Delete,
            None,
            Arc::new(AtomicBool::new(false)),
            Channel::new(|_| Ok(())),
        );

        let code = outcome
            .error
            .as_ref()
            .map(|err| err.code.clone())
            .or_else(|| outcome.failed.first().map(|item| item.code.clone()));
        assert_eq!(code.as_deref(), Some("NOT_FOUND"), "outcome: {outcome:?}");
        assert_eq!(
            outcome.failed_total + u32::from(outcome.error.is_some()),
            1,
            "outcome: {outcome:?}"
        );

        let _ = fs::remove_dir_all(&dir);
        unsafe {
            CoUninitialize();
        }
    }
}
