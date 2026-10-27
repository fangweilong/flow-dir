//! Reclaims WebView2 memory while the window is not usable.
//!
//! A minimized window keeps its WebView2 renderer resident; `TrySuspend` parks that
//! renderer and `Resume` wakes it, which is the only supported way to drop the memory
//! without destroying the webview (and with it the whole UI state). Trimming the
//! working set of our own process on the same edge keeps the task-manager numbers
//! honest without touching committed memory.
//!
//! `TrySuspend` only succeeds while the controller is invisible, so visibility is
//! toggled around it.
//!
//! Parking and waking are event driven. A parked controller renders nothing, so every
//! millisecond the app spends noticing that the window is back is a blank window; a
//! timer alone would keep it blank for up to its whole interval after the restore.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use tauri::{AppHandle, Manager, WindowEvent};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2TrySuspendCompletedHandler, ICoreWebView2_3,
};
use windows::core::Interface;
use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};

/// Browser arguments for the WebView2 environment.
///
/// Setting any arguments replaces wry's defaults, so the defaults are restated here.
/// `--disable-gpu` trades the GPU process (~110 MB private) for software compositing;
/// on this UI it costs no measurable frame time, but it stays opt-in per user setting
/// and therefore needs a restart to take effect.
pub fn browser_args(gpu_accel: bool) -> String {
    let mut args = String::from(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
         --js-flags=--max-old-space-size=256 --renderer-process-limit=1 --disk-cache-size=33554432",
    );
    if !gpu_accel {
        args.push_str(" --disable-gpu");
    }
    // Dev-only inspector endpoint; release builds carry no debugging surface at all.
    if cfg!(debug_assertions) {
        args.push_str(" --remote-debugging-port=9333");
    }
    args
}

/// Backstop for a state change that emits no window event. It costs two syscalls and no
/// longer sits on the restore path, so it is free to be slow.
const POLL: Duration = Duration::from_secs(2);

pub fn watch(app: AppHandle) {
    let (wake, wakes) = mpsc::channel();
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    // Restoring from the taskbar is a resize plus a focus gain; either one is enough to
    // get here before the window is painted again.
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Resized(_) | WindowEvent::Focused(_)) {
            let _ = wake.send(());
        }
    });
    std::thread::spawn(move || {
        let mut parked = false;
        loop {
            // Disconnected means the window is gone, so there is nothing left to park.
            if let Err(RecvTimeoutError::Disconnected) = wakes.recv_timeout(POLL) {
                return;
            }
            let Some(window) = app.get_webview_window("main") else {
                return;
            };
            let hidden =
                window.is_minimized().unwrap_or(false) || !window.is_visible().unwrap_or(true);
            if hidden == parked {
                continue;
            }
            parked = hidden;
            let _ = window.with_webview(move |webview| unsafe {
                let controller = webview.controller();
                let core = controller.CoreWebView2().ok();
                if hidden {
                    let _ = controller.SetIsVisible(false);
                    if let Some(core) = core.as_ref() {
                        if let Ok(core) = core.cast::<ICoreWebView2_3>() {
                            let _ =
                                core.TrySuspend(None::<&ICoreWebView2TrySuspendCompletedHandler>);
                        }
                    }
                    let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
                } else {
                    if let Some(core) = core.as_ref() {
                        if let Ok(core) = core.cast::<ICoreWebView2_3>() {
                            let _ = core.Resume();
                        }
                    }
                    let _ = controller.SetIsVisible(true);
                }
            });
        }
    });
}
