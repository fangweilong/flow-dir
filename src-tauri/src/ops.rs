//! Shell file operations through `IFileOperation`.
//!
//! Every operation runs on its own STA thread: `IFileOperation` requires a
//! single-threaded apartment and `PerformOperations` blocks until the shell is
//! done, so a dispatcher thread would serialize unrelated copies for no gain.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
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
        let mut items = self.items.lock().unwrap();
        if items.len() < MAX_REPORTED_FAILURES {
            items.push(OpFailure {
                path: path.to_string(),
                code: err.code,
                detail: err.detail,
            });
        }
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
struct Outcome {
    aborted: bool,
    error: Option<ApiError>,
    /// Detailed entries, capped at {@link MAX_REPORTED_FAILURES}.
    failed: Vec<OpFailure>,
    /// Every failure, including the ones cut from `failed`.
    failed_total: u32,
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
    fingerprint: String,
    at: Instant,
}

impl OpState {
    fn begin(&self, op_id: &str) -> Arc<AtomicBool> {
        let token = Arc::new(AtomicBool::new(false));
        self.jobs
            .lock()
            .unwrap()
            .insert(op_id.to_string(), token.clone());
        token
    }

    pub(crate) fn begin_external(&self, op_id: &str) -> Arc<AtomicBool> {
        self.begin(op_id)
    }

    pub(crate) fn finish_external(&self, op_id: &str) {
        self.finish(op_id);
    }

    fn finish(&self, op_id: &str) {
        self.jobs.lock().unwrap().remove(op_id);
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

    fn record_denied(&self, request: &Request) {
        let mut denied = self.denied.lock().unwrap();
        denied.retain(|_, entry| entry.at.elapsed() < std::time::Duration::from_secs(600));
        denied.insert(
            request.op_id.clone(),
            Denied {
                fingerprint: request.fingerprint(),
                at: Instant::now(),
            },
        );
    }

    /// True exactly once for a request that matches a recorded `ACCESS_DENIED` run.
    pub fn take_denied(&self, request: &Request) -> bool {
        let mut denied = self.denied.lock().unwrap();
        let Some(entry) = denied.get(&request.op_id) else {
            return false;
        };
        if entry.fingerprint != request.fingerprint() {
            return false;
        }
        if entry.at.elapsed() >= std::time::Duration::from_secs(600) {
            denied.remove(&request.op_id);
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
    pub error: Option<ApiError>,
}

#[derive(Serialize, Deserialize)]
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
}

impl Request {
    /// Identifies the operation and its exact arguments, so an elevated retry cannot be
    /// pointed at a different target than the run that was actually denied.
    pub fn fingerprint(&self) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            self.kind,
            self.sources.join("\u{1f}"),
            self.target_dir.as_deref().unwrap_or(""),
            self.new_name.as_deref().unwrap_or(""),
            self.new_names
                .as_ref()
                .map(|names| names.join("\u{1f}"))
                .unwrap_or_default(),
            self.permanent
        )
    }
}

/// Checks a request before it touches the shell; used by both the GUI path and the
/// elevated helper, which receives the request as JSON from another process.
fn validate(request: &Request, kind: Kind) -> Result<(), ApiError> {
    if request.sources.is_empty() {
        return Err(ApiError::new("EMPTY", "no source paths"));
    }
    if kind == Kind::Rename {
        match &request.new_names {
            Some(names) if names.len() != request.sources.len() => {
                return Err(ApiError::new(
                    "BAD_ARGS",
                    "batch rename needs one new name per source",
                ));
            }
            Some(_) => {}
            None if request.new_name.is_none() => {
                return Err(ApiError::new("BAD_ARGS", "rename needs newName"));
            }
            None => {}
        }
    }
    Ok(())
}

/// Runs an operation to completion on the calling thread. This is the body of the
/// elevated helper: it has no `AppHandle`, no window and no Channel back to a webview,
/// so progress is discarded and only the final result matters.
pub fn run_headless(request: Request) -> ElevatedReport {
    let report = |error: Option<ApiError>| ElevatedReport {
        aborted: false,
        failed: Vec::new(),
        failed_total: 0,
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
    if let Err(err) = validate(&request, kind) {
        return report(Some(err));
    }
    let outcome = run_guarded(&request.kind, || {
        let sink = Channel::new(|_| Ok(()));
        run(&request, kind, None, Arc::new(AtomicBool::new(false)), sink)
    });
    ElevatedReport {
        aborted: outcome.aborted,
        failed: outcome.failed,
        failed_total: outcome.failed_total,
        error: outcome.error,
    }
}

/// Starts an operation on a fresh STA thread and returns immediately.
pub fn start(
    state: &OpState,
    request: Request,
    hwnd: Option<isize>,
    progress: Channel<OpProgress>,
) -> Result<(), ApiError> {
    let kind = Kind::parse(&request.kind)
        .ok_or_else(|| ApiError::new("BAD_KIND", format!("unknown operation {}", request.kind)))?;
    validate(&request, kind)?;
    let token = state.begin(&request.op_id);
    let op_id = request.op_id.clone();
    let state = state.clone();

    std::thread::Builder::new()
        .name(format!("flowdir-op-{op_id}"))
        .spawn(move || {
            let total = request.sources.len() as u32;
            let outcome = run_guarded(&request.kind, || {
                run(&request, kind, hwnd, token, progress.clone())
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
                done: true,
                aborted: outcome.aborted,
                error: outcome.error,
            });
            if denied {
                state.record_denied(&request);
            }
            state.finish(&op_id);
        })
        .map(|_| ())
        .map_err(|e| ApiError::new("THREAD", e.to_string()))
}

fn outcome_denied(outcome: &Outcome) -> bool {
    outcome.failed.iter().any(|item| item.code == "ACCESS_DENIED")
}

/// Runs the operation body and turns a panic into a normal failure: `finish` still runs,
/// so the job cannot stay in the map (and in the taskbar progress) forever.
fn run_guarded<F: FnOnce() -> Outcome>(kind: &str, work: F) -> Outcome {    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)) {
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
) -> Outcome {
    unsafe {
        if let Err(e) = CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok() {
            return Outcome {
                aborted: false,
                error: Some(ApiError::new("COM", e.to_string())),
                failed: Vec::new(),
                failed_total: 0,
            };
        }
        let outcome = perform(request, kind, hwnd, cancel, progress);
        CoUninitialize();
        outcome
    }
}

fn shell_item(path: &str) -> Result<IShellItem, ApiError> {
    let wide = HSTRING::from(path);
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
    path.rsplit(|c| c == '\\' || c == '/')
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
        cancel,
        progress,
    };

    unsafe {
        let failure = |error: ApiError| Outcome {
            aborted: false,
            error: Some(error),
            failed: failures.take(),
            failed_total: failures.count(),
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
        let mut flags = FOF_SILENT | FOF_WANTNUKEWARNING | FOF_NOERRORUI;
        if !request.permanent {
            flags |= FOF_ALLOWUNDO;
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

        let queued = queue(&operation, request, kind, &failures);
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
        Outcome {
            aborted,
            error,
            failed: failures.take(),
            failed_total,
        }
    }
}

fn queue(
    operation: &IFileOperation,
    request: &Request,
    kind: Kind,
    failures: &FailureLog,
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
    cancel: Arc<AtomicBool>,
    progress: Channel<OpProgress>,
}

impl Sink {
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
        if let Some(path) = item.and_then(item_path) {
            *self.current_path.lock().unwrap() = path;
        }
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
        self.check_cancel()?;
        if hr.is_err() && !self.cancel.load(Ordering::Relaxed) {
            self.failures.record(&self.current_path.lock().unwrap(), hr);
            self.emit(None, true);
        }
        Ok(())
    }
}

impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn StartOperations(&self) -> WinResult<()> {
        self.emit(None, true);
        Ok(())
    }

    fn FinishOperations(&self, hr: HRESULT) -> WinResult<()> {
        // `PerformOperations` can return S_OK with a failing final HRESULT here.
        if hr.is_err() {
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
        _new: windows::core::Ref<'_, IShellItem>,
    ) -> WinResult<()> {
        self.post_item(hr)
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
        _new: windows::core::Ref<'_, IShellItem>,
    ) -> WinResult<()> {
        self.post_item(hr)
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
        _new: windows::core::Ref<'_, IShellItem>,
    ) -> WinResult<()> {
        self.post_item(hr)
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
    fn headless_rename_reports_success() {
        let dir = temp_dir("headless");
        let source = dir.join("before.txt");
        fs::write(&source, b"x").unwrap();
        let report = run_headless(Request {
            op_id: "headless".into(),
            kind: "rename".into(),
            sources: vec![source.to_string_lossy().into_owned()],
            target_dir: None,
            new_name: Some("after.txt".into()),
            new_names: None,
            permanent: false,
        });
        assert!(report.error.is_none(), "{:?}", report.error);
        assert_eq!(report.failed_total, 0);
        assert!(!report.aborted);
        assert!(dir.join("after.txt").exists());
        assert!(!source.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn headless_rejects_an_unknown_kind() {
        let report = run_headless(Request {
            op_id: "headless".into(),
            kind: "explode".into(),
            sources: vec!["C:\\".into()],
            target_dir: None,
            new_name: None,
            new_names: None,
            permanent: false,
        });
        assert_eq!(report.error.unwrap().code, "BAD_KIND");
    }

    #[test]
    fn batch_rename_rejects_mismatched_names() {
        let channel = Channel::new(|_| Ok(()));
        let request = Request {
            op_id: "bad-batch".into(),
            kind: "rename".into(),
            sources: vec!["a".into(), "b".into()],
            target_dir: None,
            new_name: None,
            new_names: Some(vec!["one".into()]),
            permanent: false,
        };
        let error = start(&OpState::default(), request, None, channel).unwrap_err();
        assert_eq!(error.code, "BAD_ARGS");
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
        };
        start(&OpState::default(), request, None, channel).unwrap();
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
