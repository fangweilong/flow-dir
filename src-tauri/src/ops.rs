//! Shell file operations through `IFileOperation`.
//!
//! Every operation runs on its own STA thread: `IFileOperation` requires a
//! single-threaded apartment and `PerformOperations` blocks until the shell is
//! done, so a dispatcher thread would serialize unrelated copies for no gain.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use tauri::ipc::Channel;
use windows::core::{implement, Result as WinResult, HSTRING, PCWSTR};
use windows::Win32::Foundation::{E_ABORT, HWND};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{
    FileOperation, IFileOperation, IFileOperationProgressSink, IFileOperationProgressSink_Impl,
    IShellItem, SHCreateItemFromParsingName, FOFX_RECYCLEONDELETE, FOF_ALLOWUNDO, FOF_SILENT,
    FOF_WANTNUKEWARNING,
};

use crate::error::ApiError;

const PROGRESS_INTERVAL_MS: u128 = 80;
const PHASES: [&str; 6] = ["start", "copy", "move", "delete", "rename", "newdir"];

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
    pub done: bool,
    pub aborted: bool,
    pub error: Option<ApiError>,
}

#[derive(Clone, Default)]
pub struct OpState {
    jobs: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
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

    fn finish(&self, op_id: &str) {
        self.jobs.lock().unwrap().remove(op_id);
    }

    pub fn cancel(&self, op_id: &str) {
        if let Some(token) = self.jobs.lock().unwrap().get(op_id) {
            token.store(true, Ordering::Relaxed);
        }
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

pub struct Request {
    pub op_id: String,
    pub kind: String,
    pub sources: Vec<String>,
    pub target_dir: Option<String>,
    pub new_name: Option<String>,
    pub permanent: bool,
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
    if request.sources.is_empty() {
        return Err(ApiError::new("EMPTY", "no source paths"));
    }
    let token = state.begin(&request.op_id);
    let op_id = request.op_id.clone();
    let state = state.clone();

    std::thread::Builder::new()
        .name(format!("flowdir-op-{op_id}"))
        .spawn(move || {
            let result = run(&request, kind, hwnd, token, progress.clone());
            let (aborted, error) = match result {
                Ok(aborted) => (aborted, None),
                Err(err) => (false, Some(err)),
            };
            let _ = progress.send(OpProgress {
                op_id: op_id.clone(),
                phase: "done".into(),
                current: 0,
                total: 0,
                name: None,
                work: 0,
                work_total: 0,
                done: true,
                aborted,
                error,
            });
            state.finish(&op_id);
        })
        .map(|_| ())
        .map_err(|e| ApiError::new("THREAD", e.to_string()))
}

fn run(
    request: &Request,
    kind: Kind,
    hwnd: Option<isize>,
    cancel: Arc<AtomicBool>,
    progress: Channel<OpProgress>,
) -> Result<bool, ApiError> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| ApiError::new("COM", e.to_string()))?;
        let result = perform(request, kind, hwnd, cancel, progress);
        CoUninitialize();
        result
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

fn perform(
    request: &Request,
    kind: Kind,
    hwnd: Option<isize>,
    cancel: Arc<AtomicBool>,
    progress: Channel<OpProgress>,
) -> Result<bool, ApiError> {
    let total = request.sources.len() as u32;
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
        cancel,
        progress,
    };

    unsafe {
        let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)
            .map_err(|e| ApiError::new("SHELL_FAILED", e.to_string()))?;
        if let Some(raw) = hwnd {
            let _ = operation.SetOwnerWindow(HWND(raw as *mut core::ffi::c_void));
        }
        let mut flags = FOF_SILENT | FOF_WANTNUKEWARNING;
        if !request.permanent {
            flags |= FOF_ALLOWUNDO;
        }
        if kind == Kind::Delete && !request.permanent {
            flags |= FOFX_RECYCLEONDELETE;
        }
        operation
            .SetOperationFlags(flags)
            .map_err(|e| ApiError::new("SHELL_FAILED", e.to_string()))?;

        let sink_iface: IFileOperationProgressSink = sink.into();
        let cookie = operation
            .Advise(&sink_iface)
            .map_err(|e| ApiError::new("SHELL_FAILED", e.to_string()))?;

        let queued = queue(&operation, request, kind);
        let result = match queued {
            Ok(()) => operation
                .PerformOperations()
                .map_err(|e| ApiError::from_windows("", e.code().0)),
            Err(err) => Err(err),
        };
        let aborted = operation
            .GetAnyOperationsAborted()
            .map(|value| value.as_bool())
            .unwrap_or(false);
        let _ = operation.Unadvise(cookie);

        match result {
            Ok(()) => Ok(aborted),
            Err(err) if aborted => {
                let _ = err;
                Ok(true)
            }
            Err(err) => Err(err),
        }
    }
}

fn queue(operation: &IFileOperation, request: &Request, kind: Kind) -> Result<(), ApiError> {
    unsafe {
        if kind == Kind::NewDir {
            let folder = shell_item(
                request
                    .target_dir
                    .as_deref()
                    .ok_or_else(|| ApiError::new("BAD_ARGS", "newdir needs targetDir"))?,
            )?;
            let name = HSTRING::from(
                request
                    .new_name
                    .as_deref()
                    .ok_or_else(|| ApiError::new("BAD_ARGS", "newdir needs newName"))?,
            );
            operation
                .NewItem(
                    &folder,
                    0x10,
                    &name,
                    PCWSTR::null(),
                    None::<&IFileOperationProgressSink>,
                )
                .map_err(|e| ApiError::from_windows("", e.code().0))?;
            return Ok(());
        }

        let target = match request.target_dir.as_deref() {
            Some(dir) => Some(shell_item(dir)?),
            None => None,
        };
        let new_name = request.new_name.as_deref().map(HSTRING::from);

        for path in &request.sources {
            let item = shell_item(path)?;
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
                Kind::Rename => operation.RenameItem(
                    &item,
                    new_name
                        .as_ref()
                        .ok_or_else(|| ApiError::new("BAD_ARGS", "rename needs newName"))?,
                    None::<&IFileOperationProgressSink>,
                ),
                Kind::NewDir => unreachable!(),
            };
            queued.map_err(|e| ApiError::from_windows(path, e.code().0))?;
        }
        Ok(())
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

    fn enter_item(&self, phase: &'static str, name: &PCWSTR) -> WinResult<()> {
        self.check_cancel()?;
        self.phase.store(
            PHASES.iter().position(|p| *p == phase).unwrap_or(0) as u8,
            Ordering::Relaxed,
        );
        self.current.fetch_add(1, Ordering::Relaxed);
        let label = if name.is_null() {
            None
        } else {
            Some(unsafe { name.to_string() }.unwrap_or_default())
        };
        self.emit(label, true);
        Ok(())
    }
}

impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn StartOperations(&self) -> WinResult<()> {
        self.emit(None, true);
        Ok(())
    }

    fn FinishOperations(&self, _hr: windows::core::HRESULT) -> WinResult<()> {
        Ok(())
    }

    fn PreRenameItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
        name: &PCWSTR,
    ) -> WinResult<()> {
        self.enter_item("rename", name)
    }

    fn PostRenameItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
        _name: &PCWSTR,
        _hr: windows::core::HRESULT,
        _new: windows::core::Ref<'_, IShellItem>,
    ) -> WinResult<()> {
        self.check_cancel()
    }

    fn PreMoveItem(
        &self,
        _flags: u32,
        item: windows::core::Ref<'_, IShellItem>,
        _dir: windows::core::Ref<'_, IShellItem>,
        name: &PCWSTR,
    ) -> WinResult<()> {
        let _ = item;
        self.enter_item("move", name)
    }

    fn PostMoveItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
        _dir: windows::core::Ref<'_, IShellItem>,
        _name: &PCWSTR,
        _hr: windows::core::HRESULT,
        _new: windows::core::Ref<'_, IShellItem>,
    ) -> WinResult<()> {
        self.check_cancel()
    }

    fn PreCopyItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
        _dir: windows::core::Ref<'_, IShellItem>,
        name: &PCWSTR,
    ) -> WinResult<()> {
        self.enter_item("copy", name)
    }

    fn PostCopyItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
        _dir: windows::core::Ref<'_, IShellItem>,
        _name: &PCWSTR,
        _hr: windows::core::HRESULT,
        _new: windows::core::Ref<'_, IShellItem>,
    ) -> WinResult<()> {
        self.check_cancel()
    }

    fn PreDeleteItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
    ) -> WinResult<()> {
        self.enter_item("delete", &PCWSTR::null())
    }

    fn PostDeleteItem(
        &self,
        _flags: u32,
        _item: windows::core::Ref<'_, IShellItem>,
        _hr: windows::core::HRESULT,
        _new: windows::core::Ref<'_, IShellItem>,
    ) -> WinResult<()> {
        let _ = _new;
        self.check_cancel()
    }

    fn PreNewItem(
        &self,
        _flags: u32,
        _dir: windows::core::Ref<'_, IShellItem>,
        name: &PCWSTR,
    ) -> WinResult<()> {
        self.enter_item("newdir", name)
    }

    fn PostNewItem(
        &self,
        _flags: u32,
        _dir: windows::core::Ref<'_, IShellItem>,
        _name: &PCWSTR,
        _template: &PCWSTR,
        _attrs: u32,
        _hr: windows::core::HRESULT,
        _new: windows::core::Ref<'_, IShellItem>,
    ) -> WinResult<()> {
        self.check_cancel()
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
