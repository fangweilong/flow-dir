//! Taskbar progress and completion attention for long file operations.
//!
//! The window is created in Rust, so the taskbar button belongs to it: `ITaskbarList3`
//! needs that HWND, and every call runs on its own STA thread because the COM object is
//! apartment-threaded while Tauri commands arrive on worker threads.

use std::ffi::c_void;
use std::sync::mpsc::{self, SyncSender};
use std::sync::LazyLock;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{
    ITaskbarList3, TaskbarList, TBPF_ERROR, TBPF_NOPROGRESS, TBPF_NORMAL, TBPF_PAUSED, TBPFLAG,
};
use windows::Win32::UI::WindowsAndMessaging::{FlashWindowEx, FLASHWINFO, FLASHW_TRAY};

use crate::error::ApiError;

struct TaskbarMsg {
    hwnd: isize,
    flag: TBPFLAG,
    clear: bool,
    completed: u64,
    total: u64,
}

static TASKBAR_TX: LazyLock<Option<SyncSender<TaskbarMsg>>> = LazyLock::new(|| {
    let (tx, rx) = mpsc::sync_channel::<TaskbarMsg>(32);
    let spawned = std::thread::Builder::new()
        .name("flowdir-taskbar".into())
        .spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let table: Option<ITaskbarList3> = (|| -> windows::core::Result<ITaskbarList3> {
                let table: ITaskbarList3 = CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER)?;
                table.HrInit()?;
                Ok(table)
            })()
            .ok();
            if let Some(table) = table {
                while let Ok(msg) = rx.recv() {
                    let hwnd = HWND(msg.hwnd as *mut c_void);
                    let _ = table.SetProgressState(hwnd, msg.flag);
                    if !msg.clear {
                        let _ = table.SetProgressValue(hwnd, msg.completed, msg.total.max(1));
                    }
                }
            }
            CoUninitialize();
        });
    if spawned.is_ok() {
        Some(tx)
    } else {
        None
    }
});

fn flag(state: &str) -> Option<TBPFLAG> {
    match state {
        "normal" => Some(TBPF_NORMAL),
        "error" => Some(TBPF_ERROR),
        "paused" => Some(TBPF_PAUSED),
        "none" => Some(TBPF_NOPROGRESS),
        _ => None,
    }
}

/// Sets the taskbar progress. `none` clears it; `completed`/`total` are item counts.
pub fn progress(hwnd: isize, state: &str, completed: u64, total: u64) -> Result<(), ApiError> {
    let Some(flag) = flag(state) else {
        return Err(ApiError::new("BAD_STATE", state));
    };
    let clear = state == "none";
    if let Some(tx) = TASKBAR_TX.as_ref() {
        let _ = tx.try_send(TaskbarMsg {
            hwnd,
            flag,
            clear,
            completed,
            total,
        });
    }
    Ok(())
}

/// Flashes the taskbar button; used when an operation finishes while the app is in the
/// background, where the in-app toast is invisible.
pub fn flash(hwnd: isize) {
    let info = FLASHWINFO {
        cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
        hwnd: HWND(hwnd as *mut c_void),
        dwFlags: FLASHW_TRAY,
        uCount: 3,
        dwTimeout: 0,
    };
    unsafe {
        let _ = FlashWindowEx(&info);
    }
}
