use std::ptr;
use std::sync::Mutex;

use windows::core::{w, BOOL};
use windows::Win32::Foundation::{GlobalFree, HANDLE, POINT};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardSequenceNumber, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::CF_HDROP;
use windows::Win32::UI::Shell::DROPFILES;

use crate::error::ApiError;

const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;

/// A WebDAV path has no `CF_HDROP` form, and writing it anyway would make Explorer paste
/// a literal `webdav://…` filename. The full selection therefore lives here while the OS
/// clipboard receives only the local subset. `sequence` is the clipboard sequence number
/// right after that write: once another app changes the clipboard, the internal copy is
/// stale and must not resurface.
struct InternalClip {
    clip: Clip,
    sequence: u32,
}

static INTERNAL: Mutex<Option<InternalClip>> = Mutex::new(None);

fn open_clipboard_with_retry() -> windows::core::Result<()> {
    for _ in 0..5 {
        if unsafe { OpenClipboard(None) }.is_ok() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    unsafe { OpenClipboard(None) }
}

/// Puts `paths` on the clipboard as `CF_HDROP` so Explorer can paste them. A selection
/// containing WebDAV paths keeps the complete list in-process (see [`InternalClip`]).
pub fn copy_paths(paths: &[String], cut: bool) -> Result<(), ApiError> {
    if paths.is_empty() {
        return Err(ApiError::new("EMPTY", "没有选中任何项目"));
    }
    // Shell-namespace items have no filesystem path, so `CF_HDROP` cannot represent them.
    // Rejecting the whole selection is honest: silently dropping them from a mixed
    // selection would report success while copying only part of what the user selected.
    if paths
        .iter()
        .any(|path| crate::shellfs::is_shell_path(path))
    {
        return Err(ApiError::new(
            "INVALID_PATH",
            "shell namespace items cannot be copied as files",
        ));
    }
    let local: Vec<String> = paths
        .iter()
        .filter(|path| !crate::webdav::is_webdav(path))
        .cloned()
        .collect();
    let has_remote = local.len() != paths.len();

    // Hold the internal lock across the clipboard write. Otherwise a concurrent paste can
    // observe the new `CF_HDROP` (local subset only) before the full mixed selection is
    // stored and silently drop the WebDAV items.
    // `write_os_paths` captures the sequence after the clipboard is closed; `read_paths`
    // compares it with the current number to decide whether the internal selection is
    // still current.
    let mut internal = INTERNAL.lock().unwrap();
    let sequence = write_os_paths(&local, cut)?;
    if has_remote {
        *internal = Some(InternalClip {
            clip: Clip {
                paths: paths.to_vec(),
                cut,
            },
            sequence,
        });
    } else {
        *internal = None;
    }
    Ok(())
}

/// Writes the shell-compatible subset. An empty list still empties the clipboard, so a
/// remote-only copy cannot leave an unrelated file list behind for a later paste.
fn write_os_paths(paths: &[String], cut: bool) -> Result<u32, ApiError> {
    let mut wide: Vec<u16> = Vec::new();
    for path in paths {
        wide.extend(path.encode_utf16());
        wide.push(0);
    }
    wide.push(0);

    let header = std::mem::size_of::<DROPFILES>();
    let size = header + wide.len() * 2;

    unsafe {
        open_clipboard_with_retry().map_err(|e| ApiError::new("CLIPBOARD", e.to_string()))?;
        let result = (|| -> windows::core::Result<()> {
            EmptyClipboard()?;
            if paths.is_empty() {
                return Ok(());
            }
            let hglobal = GlobalAlloc(GMEM_MOVEABLE, size)?;
            let target = GlobalLock(hglobal);
            if target.is_null() {
                let _ = GlobalFree(Some(hglobal));
                return Err(windows::core::Error::from_thread());
            }
            ptr::write_bytes(target as *mut u8, 0, size);
            let dropfiles = target as *mut DROPFILES;
            ptr::write(
                dropfiles,
                DROPFILES {
                    pFiles: header as u32,
                    pt: POINT { x: 0, y: 0 },
                    fNC: BOOL(0),
                    fWide: BOOL(1),
                },
            );
            ptr::copy_nonoverlapping(
                wide.as_ptr(),
                (target as *mut u8).add(header).cast(),
                wide.len(),
            );
            let _ = GlobalUnlock(hglobal);
            // SetClipboardData takes ownership only on success; otherwise the block leaks.
            if let Err(err) = SetClipboardData(CF_HDROP.0 as u32, Some(HANDLE(hglobal.0))) {
                let _ = GlobalFree(Some(hglobal));
                return Err(err);
            }

            let effect_format = RegisterClipboardFormatW(w!("Preferred DropEffect"));
            if effect_format != 0 {
                let effect = if cut {
                    DROPEFFECT_MOVE
                } else {
                    DROPEFFECT_COPY
                };
                let hmem = GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of::<u32>())?;
                let ptr_effect = GlobalLock(hmem);
                if ptr_effect.is_null() {
                    let _ = GlobalFree(Some(hmem));
                } else {
                    ptr::write(ptr_effect as *mut u32, effect);
                    let _ = GlobalUnlock(hmem);
                    if let Err(err) = SetClipboardData(effect_format, Some(HANDLE(hmem.0))) {
                        let _ = GlobalFree(Some(hmem));
                        return Err(err);
                    }
                }
            }
            Ok(())
        })();
        // Read the sequence after closing. Closing can advance the sequence (directly or
        // through clipboard clients reacting to the update), so a value captured before it
        // would not match what `read_paths` sees later and the internal WebDAV selection
        // would be discarded on the first paste.
        let _ = CloseClipboard();
        let sequence = GetClipboardSequenceNumber();
        result
            .map(|()| sequence)
            .map_err(|e| ApiError::new("CLIPBOARD", e.to_string()))
    }
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    pub paths: Vec<String>,
    pub cut: bool,
}

/// Reads `CF_HDROP` (plus `Preferred DropEffect`) written by Explorer or by us. A still
/// current in-process WebDAV selection takes precedence because it also carries the remote
/// paths that `CF_HDROP` could not represent.
pub fn read_paths() -> Result<Clip, ApiError> {
    use windows::Win32::System::DataExchange::GetClipboardData;
    use windows::Win32::UI::Shell::DragQueryFileW;

    let sequence = unsafe { GetClipboardSequenceNumber() };
    {
        let mut internal = INTERNAL.lock().unwrap();
        if let Some(stored) = internal.as_ref() {
            if stored.sequence == sequence {
                return Ok(stored.clip.clone());
            }
            *internal = None;
        }
    }

    unsafe {
        open_clipboard_with_retry().map_err(|e| ApiError::new("CLIPBOARD", e.to_string()))?;
        let result = (|| -> windows::core::Result<Clip> {
            let handle = GetClipboardData(CF_HDROP.0 as u32)?;
            let drop = windows::Win32::UI::Shell::HDROP(handle.0);
            let count = DragQueryFileW(drop, 0xFFFF_FFFF, None);
            let mut paths = Vec::with_capacity(count as usize);
            for index in 0..count {
                let len = DragQueryFileW(drop, index, None) as usize;
                let mut buffer = vec![0u16; len + 1];
                let written = DragQueryFileW(drop, index, Some(&mut buffer)) as usize;
                if written == 0 {
                    continue;
                }
                buffer.truncate(written);
                paths.push(String::from_utf16_lossy(&buffer));
            }
            let mut cut = false;
            let format = RegisterClipboardFormatW(w!("Preferred DropEffect"));
            if format != 0 {
                if let Ok(effect) = GetClipboardData(format) {
                    let hglobal = windows::Win32::Foundation::HGLOBAL(effect.0);
                    // Read the drop effect only when the block can actually hold one.
                    if GlobalSize(hglobal) >= std::mem::size_of::<u32>() {
                        let ptr = windows::Win32::System::Memory::GlobalLock(hglobal);
                        if !ptr.is_null() {
                            cut = (ptr as *const u32).read_unaligned() == DROPEFFECT_MOVE;
                            let _ = windows::Win32::System::Memory::GlobalUnlock(hglobal);
                        }
                    }
                }
            }
            Ok(Clip { paths, cut })
        })();
        let _ = CloseClipboard();
        result.map_err(|e| ApiError::new("CLIPBOARD", e.to_string()))
    }
}
