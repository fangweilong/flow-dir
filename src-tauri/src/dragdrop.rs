//! Windows OLE drop target for drags coming from other applications.
//!
//! WebView2's/wry's drop target only understands `CF_HDROP`, so a drag that carries file
//! paths as text is reported as "not allowed" and never reaches the app. VS Code is such a
//! source: its Explorer drag puts the absolute paths in `text/plain` (plus VS Code-specific
//! formats) but offers no `CF_HDROP`. Replacing the target lets us accept `CF_HDROP` from
//! Explorer and every other shell source, and fall back to a textual path list.
//!
//! The frontend still decides where the files go; this module only extracts paths and
//! forwards pointer positions using the payload shape Tauri's `onDragDropEvent` produced.

use std::cell::{RefCell, UnsafeCell};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use percent_encoding::percent_decode_str;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use windows::core::{implement, Ref, Result as WinResult, BOOL};
use windows::Win32::Foundation::{DRAGDROP_E_INVALIDHWND, HWND, LPARAM, POINT, POINTL};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop, CF_HDROP,
    CF_UNICODETEXT, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragFinish, DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::EnumChildWindows;

/// Event consumed by `useDragAndDrop`; the payload mirrors Tauri's native drag event.
const DRAG_EVENT: &str = "flowdir:drag-drop";

/// Text larger than this cannot be a path list and must not be scanned line by line.
const MAX_TEXT_BYTES: usize = 64 * 1024;

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

/// Reads every path from the drag's `CF_HDROP` payload.
///
/// `finish` releases the handle with `DragFinish` once the drop is done; during
/// `DragEnter` the source keeps ownership so a later render still succeeds.
fn hdrop_paths(data: &IDataObject, finish: bool) -> Vec<PathBuf> {
    let medium = unsafe { data.GetData(&format(CF_HDROP.0)) };
    let Ok(medium) = medium else {
        return Vec::new();
    };
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
    if finish {
        unsafe { DragFinish(hdrop) };
    }
    paths
}

/// Copies the drag's `CF_UNICODETEXT` payload.
fn unicode_text(data: &IDataObject) -> Option<String> {
    let mut medium = unsafe { data.GetData(&format(CF_UNICODETEXT.0)) }.ok()?;
    let text = unsafe {
        let handle = medium.u.hGlobal;
        let ptr = GlobalLock(handle) as *const u16;
        if ptr.is_null() {
            None
        } else {
            let mut len = 0usize;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(handle);
            Some(text)
        }
    };
    unsafe { ReleaseStgMedium(&mut medium) };
    text
}

/// Turns a `file:` URI from a text payload into a local path.
///
/// Some drag sources write URI strings instead of filesystem paths. Only the Windows
/// forms (empty host, `localhost`, or a drive path) are understood; anything else has no
/// local file and is left for the caller to reject.
fn file_uri_path(line: &str) -> Option<PathBuf> {
    let rest = line.get(..7)?;
    if !rest.eq_ignore_ascii_case("file://") {
        return None;
    }
    let decoded = percent_decode_str(&line[7..]).decode_utf8().ok()?;
    let path = if let Some(path) = decoded.strip_prefix("localhost/") {
        path
    } else if let Some(path) = decoded.strip_prefix('/') {
        path
    } else {
        return None;
    };
    let path = Path::new(path);
    path.is_absolute().then(|| path.to_path_buf())
}

/// Reads a drag whose paths were only published as plain text.
///
/// Editors such as VS Code send `text/plain` with one absolute path per line. Only accept
/// the text when every non-empty line names an existing absolute path, so a dragged code
/// snippet that happens to contain a path is not turned into a file transfer.
fn parse_text_paths(text: &str) -> Vec<PathBuf> {
    if text.len() > MAX_TEXT_BYTES {
        return Vec::new();
    }
    let mut paths = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let path = file_uri_path(line).unwrap_or_else(|| PathBuf::from(line));
        if !path.is_absolute() || std::fs::symlink_metadata(&path).is_err() {
            return Vec::new();
        }
        paths.push(path);
    }
    paths
}

fn text_paths(data: &IDataObject) -> Vec<PathBuf> {
    unicode_text(data)
        .map(|text| parse_text_paths(&text))
        .unwrap_or_default()
}

fn drag_paths(data: &IDataObject, finish: bool) -> Vec<PathBuf> {
    let paths = hdrop_paths(data, finish);
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
        let paths = drag_paths(data, false);
        let valid = !paths.is_empty();
        unsafe { *self.enter_is_valid.get() = valid };
        if !valid {
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
        _pdwEffect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        if unsafe { *self.enter_is_valid.get() } {
            if let Some(data) = pDataObj.as_ref() {
                let paths = drag_paths(data, true);
                if !paths.is_empty() {
                    self.emit(DragPayload::Drop {
                        paths: strings(paths),
                        position: self.position(pt),
                    });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> (PathBuf, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("flowdir-dragdrop-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("file.txt");
        std::fs::write(&file, b"x").unwrap();
        (dir, file)
    }

    #[test]
    fn text_path_list_accepts_existing_absolute_paths() {
        let (dir, file) = temp_file("accept");
        let text = format!(
            "{}\r\n{}\r\n",
            file.to_string_lossy(),
            dir.to_string_lossy()
        );
        let paths = parse_text_paths(&text);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], file);
        assert_eq!(paths[1], dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_path_list_rejects_prose_and_relative_paths() {
        for text in ["hello world", "file.txt\r\n", "https://example.invalid/x"] {
            assert!(parse_text_paths(text).is_empty());
        }
    }

    #[test]
    fn text_path_list_accepts_file_uris() {
        let (dir, file) = temp_file("uri");
        let uri = format!("file:///{}", file.to_string_lossy().replace('\\', "/"));
        assert_eq!(parse_text_paths(&uri), vec![file]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_path_list_rejects_mixed_valid_and_invalid_lines() {
        let (dir, file) = temp_file("mixed");
        let text = format!("{}\r\nnot a path\r\n", file.to_string_lossy());
        assert!(parse_text_paths(&text).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
