//! Windows OLE drop target for drags coming from other applications.
//!
//! WebView2's/wry's drop target only understands `CF_HDROP`, so a drag that carries file
//! paths as text is reported as "not allowed" and never reaches the app. VS Code is such a
//! source: its Explorer drag puts the absolute paths in `text/plain` (plus VS Code-specific
//! formats) but offers no `CF_HDROP`. Java applications (IntelliJ) are another: depending on
//! the drag source they publish `CF_HDROP`, `FileNameW`, `text/uri-list` or plain text.
//! Replacing the target lets us read every one of those spellings.
//!
//! The frontend still decides where the files go; this module only extracts paths and
//! forwards pointer positions using the payload shape Tauri's `onDragDropEvent` produced.

use std::cell::{RefCell, UnsafeCell};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use windows::core::{implement, w, Ref, Result as WinResult, BOOL, PCWSTR};
use windows::Win32::Foundation::{DRAGDROP_E_INVALIDHWND, HGLOBAL, HWND, LPARAM, POINT, POINTL};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{
    IDataObject, DATADIR_GET, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::{GetClipboardFormatNameW, RegisterClipboardFormatW};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop, CF_HDROP,
    CF_UNICODETEXT, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::EnumChildWindows;

use crate::pathlist::{list_from_hglobal, parse_text_paths, parse_uri_list, text_from_hglobal};

/// Event consumed by `useDragAndDrop`; the payload mirrors Tauri's native drag event.
const DRAG_EVENT: &str = "flowdir:drag-drop";

thread_local! {
  /// OLE keeps the targets alive while they are registered, but holding them here makes
  /// the lifetime explicit and keeps the vtables reachable for the process lifetime.
  static TARGETS: RefCell<Vec<IDropTarget>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone, Copy, Serialize)]
struct DragPosition {
    x: f64,
    y: f64,
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum DragPayload {
    Enter {
        paths: Vec<String>,
        position: DragPosition,
    },
    Over {
        position: DragPosition,
    },
    Drop {
        paths: Vec<String>,
        position: DragPosition,
    },
    Leave,
}

#[implement(IDropTarget)]
struct FlowDirDropTarget {
    hwnd: HWND,
    app: AppHandle,
    cursor_effect: UnsafeCell<DROPEFFECT>,
    /// Whether the current drag carries paths; OLE calls Over/Leave/Drop unconditionally,
    /// but the frontend must not receive events for a drag we refused in DragEnter.
    enter_is_valid: UnsafeCell<bool>,
    /// Set once a drag has been refused for its format, so the formats of every later
    /// unrecognised drag are not logged again; a source we cannot read should be
    /// diagnosable from one log line, not from a flood.
    format_reported: UnsafeCell<bool>,
}

impl FlowDirDropTarget {
    fn position(&self, pt: &POINTL) -> DragPosition {
        let mut point = POINT { x: pt.x, y: pt.y };
        let _ = unsafe { ScreenToClient(self.hwnd, &mut point) };
        DragPosition {
            x: point.x as f64,
            y: point.y as f64,
        }
    }

    fn emit(&self, payload: DragPayload) {
        let _ = self.app.emit(DRAG_EVENT, payload);
    }

    /// Names the formats an unrecognised drag offered. Without this the only symptom is
    /// that the drop does nothing, which says nothing about what the source published.
    fn report_unknown(&self, data: &IDataObject) {
        if unsafe { *self.format_reported.get() } {
            return;
        }
        unsafe { *self.format_reported.get() = true };
        log::warn!(
            target: "flowdir::dragdrop",
            "drag carries no readable paths; formats: {}",
            format_names(data).join(", ")
        );
    }
}

fn format(cf: u16) -> FORMATETC {
    FORMATETC {
        cfFormat: cf,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

/// Registered clipboard format id, or `None` when the name is not registered yet (nothing
/// can be publishing it either, so the caller can skip the lookup).
fn format_id(name: PCWSTR) -> Option<u16> {
    let id = unsafe { RegisterClipboardFormatW(name) };
    (id != 0).then_some(id as u16)
}

/// Copies the medium's `HGLOBAL` payload out of the data object.
///
/// The data object hands out a copy of the medium, so it is released here in every phase
/// (also during `DragEnter`); the source's own handle is not affected.
fn take_medium<T>(data: &IDataObject, cf: u16, read: impl FnOnce(HGLOBAL) -> T) -> Option<T> {
    let mut medium = unsafe { data.GetData(&format(cf)) }.ok()?;
    if medium.tymed != TYMED_HGLOBAL.0 as u32 {
        unsafe { ReleaseStgMedium(&mut medium) };
        return None;
    }
    let payload = read(HGLOBAL(unsafe { medium.u.hGlobal.0 }));
    unsafe { ReleaseStgMedium(&mut medium) };
    Some(payload)
}

/// Reads every path from the drag's `CF_HDROP` payload.
fn hdrop_paths(data: &IDataObject) -> Vec<PathBuf> {
    let mut medium = match unsafe { data.GetData(&format(CF_HDROP.0)) } {
        Ok(medium) => medium,
        Err(_) => return Vec::new(),
    };
    if medium.tymed != TYMED_HGLOBAL.0 as u32 {
        unsafe { ReleaseStgMedium(&mut medium) };
        return Vec::new();
    }
    let hdrop = HDROP(unsafe { medium.u.hGlobal.0 } as _);
    let mut paths = Vec::new();
    loop {
        let index = paths.len() as u32;
        let len = unsafe { DragQueryFileW(hdrop, index, None) } as usize;
        if len == 0 {
            break;
        }
        let mut buf = vec![0u16; len + 1];
        let copied = unsafe { DragQueryFileW(hdrop, index, Some(&mut buf)) } as usize;
        if copied == 0 {
            break;
        }
        paths.push(PathBuf::from(OsString::from_wide(&buf[..copied])));
    }
    unsafe { ReleaseStgMedium(&mut medium) };
    paths
}

/// `FileNameW` holds a NUL-separated list of full paths; it predates `CF_HDROP` and some
/// Java and delphi-era sources still prefer it.
fn file_name_paths(data: &IDataObject) -> Vec<PathBuf> {
    let Some(id) = format_id(w!("FileNameW")) else {
        return Vec::new();
    };
    take_medium(data, id, list_from_hglobal)
        .map(|list| list.into_iter().map(PathBuf::from).collect())
        .unwrap_or_default()
}

/// Paths that were only published as text: `text/uri-list` (RFC 2483) or a plain
/// one-path-per-line list, which is what VS Code and most editors send.
fn text_paths(data: &IDataObject) -> Vec<PathBuf> {
    if let Some(id) = format_id(w!("text/uri-list")) {
        if let Some(Some(list)) = take_medium(data, id, text_from_hglobal) {
            let paths = parse_uri_list(&list);
            if !paths.is_empty() {
                return paths;
            }
        }
    }
    // The readers return `Option` themselves (a malformed block has no text), so the medium
    // lookup is flattened rather than matched twice.
    match take_medium(data, CF_UNICODETEXT.0, text_from_hglobal) {
        Some(Some(text)) => parse_text_paths(&text),
        _ => Vec::new(),
    }
}

fn drag_paths(data: &IDataObject) -> Vec<PathBuf> {
    let paths = hdrop_paths(data);
    if !paths.is_empty() {
        return paths;
    }
    let paths = file_name_paths(data);
    if !paths.is_empty() {
        return paths;
    }
    text_paths(data)
}

fn strings(paths: Vec<PathBuf>) -> Vec<String> {
    paths
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

/// Human-readable names of every format the drag offers, for the one-time warning above.
///
/// The enumeration is capped: `IEnumFORMATETC::Next` answers S_FALSE when it runs out, and a
/// source that never does would otherwise spin the thread delivering the drag.
fn format_names(data: &IDataObject) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(enumerator) = (unsafe { data.EnumFormatEtc(DATADIR_GET.0 as u32) }) else {
        return out;
    };
    for _ in 0..64 {
        let mut entry = [format(0)];
        let mut fetched = 0u32;
        let status = unsafe { enumerator.Next(&mut entry, Some(&mut fetched)) };
        if status.is_err() || fetched == 0 {
            break;
        }
        let cf = entry[0].cfFormat;
        let mut buffer = [0u16; 128];
        let len = unsafe { GetClipboardFormatNameW(cf as u32, &mut buffer) };
        let name = if len > 0 {
            String::from_utf16_lossy(&buffer[..(len as usize).min(buffer.len())])
        } else {
            format!("cf={cf}")
        };
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

#[allow(non_snake_case)]
impl IDropTarget_Impl for FlowDirDropTarget_Impl {
    fn DragEnter(
        &self,
        pDataObj: Ref<'_, IDataObject>,
        _grfKeyState: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdwEffect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        let Some(data) = pDataObj.as_ref() else {
            return Ok(());
        };
        let paths = drag_paths(data);
        let valid = !paths.is_empty();
        unsafe { *self.enter_is_valid.get() = valid };
        if !valid {
            // OLE requires the target to answer with one of the source's offered effects;
            // leaving the value untouched is what made a refused drag look like a drop that
            // silently did nothing instead of showing the no-drop cursor.
            self.report_unknown(data);
            unsafe {
                *pdwEffect = DROPEFFECT_NONE;
                *self.cursor_effect.get() = DROPEFFECT_NONE;
            }
            return Ok(());
        }
        self.emit(DragPayload::Enter {
            paths: strings(paths),
            position: self.position(pt),
        });
        unsafe {
            *pdwEffect = DROPEFFECT_COPY;
            *self.cursor_effect.get() = DROPEFFECT_COPY;
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _grfKeyState: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdwEffect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        if unsafe { *self.enter_is_valid.get() } {
            self.emit(DragPayload::Over {
                position: self.position(pt),
            });
        }
        unsafe { *pdwEffect = *self.cursor_effect.get() };
        Ok(())
    }

    fn DragLeave(&self) -> WinResult<()> {
        if unsafe { *self.enter_is_valid.get() } {
            self.emit(DragPayload::Leave);
        }
        Ok(())
    }

    fn Drop(
        &self,
        pDataObj: Ref<'_, IDataObject>,
        _grfKeyState: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdwEffect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        // The source waits on this value to decide whether to delete (move) or keep (copy)
        // what it dragged; reporting nothing makes it treat the drop as refused.
        unsafe { *pdwEffect = DROPEFFECT_NONE };
        if unsafe { *self.enter_is_valid.get() } {
            if let Some(data) = pDataObj.as_ref() {
                let paths = drag_paths(data);
                if !paths.is_empty() {
                    self.emit(DragPayload::Drop {
                        paths: strings(paths),
                        position: self.position(pt),
                    });
                    unsafe { *pdwEffect = DROPEFFECT_COPY };
                }
            }
        }
        Ok(())
    }
}

/// Replaces the drop target wry registered for this window.
///
/// wry enumerates the WebView2 child windows and overrides only those that already carry a
/// drop target; the same rule is used here. The built-in handler stays registered for any
/// window we cannot replace, so Explorer drops keep working even if this partially fails.
pub fn install(app: &AppHandle, parent: HWND) {
    let mut children: Vec<HWND> = Vec::new();
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let list = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
        list.push(hwnd);
        true.into()
    }
    let _ = unsafe {
        EnumChildWindows(
            Some(parent),
            Some(collect),
            LPARAM(&mut children as *mut Vec<HWND> as isize),
        )
    };

    let mut targets = Vec::new();
    for hwnd in children {
        let target: IDropTarget = FlowDirDropTarget {
            hwnd,
            app: app.clone(),
            cursor_effect: UnsafeCell::new(DROPEFFECT_NONE),
            enter_is_valid: UnsafeCell::new(false),
            format_reported: UnsafeCell::new(false),
        }
        .into();
        // A window without an existing target is not a drop surface; registering there would
        // intercept drags WebView2 currently routes elsewhere.
        if unsafe { RevokeDragDrop(hwnd) } != Err(DRAGDROP_E_INVALIDHWND.into())
            && unsafe { RegisterDragDrop(hwnd, &target) }.is_ok()
        {
            targets.push(target);
        }
    }
    let registered = targets.len();
    TARGETS.with(|slot| slot.borrow_mut().extend(targets));
    if registered == 0 {
        // Registration is silent; without a target only Tauri's CF_HDROP handler would see
        // drags, which is exactly the state this module exists to avoid.
        log::warn!(target: "flowdir::dragdrop", "no OLE drop target was registered");
    }
}
