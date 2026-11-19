use std::ptr;
use std::sync::Mutex;

use windows::core::{w, BOOL};
use windows::Win32::Foundation::{GlobalFree, HGLOBAL, HANDLE, POINT};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardFormatNameW, GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::{CF_HDROP, CF_TEXT, CF_UNICODETEXT};
use windows::Win32::UI::Shell::{DragQueryFileW, DROPFILES, HDROP};

use crate::error::ApiError;
use crate::pathlist::{
    list_from_hglobal, native_separators, parse_text_paths, parse_uri_list, text_from_hglobal,
};

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

/// Moves `bytes` into a clipboard-owned `GMEM_MOVEABLE` block.
///
/// # Safety
/// `bytes` must be readable for its whole length.
unsafe fn set_block(format: u32, bytes: &[u8]) -> windows::core::Result<()> {
    let hglobal = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len())? };
    let target = unsafe { GlobalLock(hglobal) };
    if target.is_null() {
        let _ = unsafe { GlobalFree(Some(hglobal)) };
        return Err(windows::core::Error::from_thread());
    }
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), target as *mut u8, bytes.len());
        let _ = GlobalUnlock(hglobal);
    }
    // SetClipboardData takes ownership only on success; otherwise the block leaks.
    if let Err(err) = unsafe { SetClipboardData(format, Some(HANDLE(hglobal.0))) } {
        let _ = unsafe { GlobalFree(Some(hglobal)) };
        return Err(err);
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

    unsafe {
        open_clipboard_with_retry().map_err(|e| ApiError::new("CLIPBOARD", e.to_string()))?;
        let result = (|| -> windows::core::Result<()> {
            EmptyClipboard()?;
            if paths.is_empty() {
                return Ok(());
            }

            // `CF_HDROP` first: this is what Explorer, other file managers and FlowDir
            // itself paste from.
            let mut drop = vec![0u8; header + wide.len() * 2];
            ptr::write(
                drop.as_mut_ptr() as *mut DROPFILES,
                DROPFILES {
                    pFiles: header as u32,
                    pt: POINT { x: 0, y: 0 },
                    fNC: BOOL(0),
                    fWide: BOOL(1),
                },
            );
            for (index, unit) in wide.iter().enumerate() {
                let offset = header + index * 2;
                drop[offset..offset + 2].copy_from_slice(&unit.to_le_bytes());
            }
            set_block(CF_HDROP.0 as u32, &drop)?;

            // Text forms beside it, so an application that cannot take a file list (an
            // editor's open dialog, a terminal, IntelliJ's paste) still receives the paths.
            // `CF_UNICODETEXT` is the format Windows text consumers actually read — a custom
            // format merely *named* `text/plain` would be written and never read.
            let text = paths.join("\r\n");
            let text_wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let text_bytes: Vec<u8> = text_wide.iter().flat_map(|unit| unit.to_le_bytes()).collect();
            // A missing text form is not a failure: the file list is what makes the paste
            // work, so a rejected side format is logged and stepped over.
            if let Err(err) = set_block(CF_UNICODETEXT.0 as u32, &text_bytes) {
                log::debug!("CF_UNICODETEXT write failed: {err}");
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

/// The block a clipboard format currently publishes, or `None` when it is absent. The
/// clipboard must be open; the handle stays owned by the clipboard.
fn block(format: u16) -> Option<HGLOBAL> {
    let handle = unsafe { GetClipboardData(format as u32) }.ok()?;
    Some(HGLOBAL(handle.0))
}

/// `Preferred DropEffect` says whether the copy was a cut. Absent means copy.
fn read_drop_effect() -> bool {
    let format = unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) };
    if format == 0 {
        return false;
    }
    let Some(hglobal) = block(format as u16) else {
        return false;
    };
    // Read the drop effect only when the block can actually hold one.
    if unsafe { GlobalSize(hglobal) } < std::mem::size_of::<u32>() {
        return false;
    }
    let ptr = unsafe { GlobalLock(hglobal) };
    if ptr.is_null() {
        return false;
    }
    let effect = unsafe { (ptr as *const u32).read_unaligned() };
    let _ = unsafe { GlobalUnlock(hglobal) };
    effect == DROPEFFECT_MOVE
}

/// Every path in the clipboard's `CF_HDROP` list.
fn hdrop_paths() -> Vec<String> {
    let Some(handle) = block(CF_HDROP.0) else {
        return Vec::new();
    };
    let drop = HDROP(handle.0);
    let count = unsafe { DragQueryFileW(drop, 0xFFFF_FFFF, None) };
    let mut paths = Vec::with_capacity(count as usize);
    for index in 0..count {
        let len = unsafe { DragQueryFileW(drop, index, None) } as usize;
        if len == 0 {
            continue;
        }
        let mut buffer = vec![0u16; len + 1];
        let written = unsafe { DragQueryFileW(drop, index, Some(&mut buffer)) } as usize;
        if written == 0 {
            continue;
        }
        buffer.truncate(written);
        paths.push(native_separators(&String::from_utf16_lossy(&buffer)));
    }
    paths
}

/// `FileName` (ANSI) is a NUL-separated list of single-byte paths; lifting each byte to a
/// code point is the only lossless read without knowing the source's code page.
fn ansi_list(hglobal: HGLOBAL) -> Vec<String> {
    let size = unsafe { GlobalSize(hglobal) } as usize;
    if size == 0 {
        return Vec::new();
    }
    let ptr = unsafe { GlobalLock(hglobal) } as *const u8;
    if ptr.is_null() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start = 0usize;
    unsafe {
        while start < size {
            let mut end = start;
            while end < size && *ptr.add(end) != 0 {
                end += 1;
            }
            if end == start {
                break;
            }
            out.push(
                std::slice::from_raw_parts(ptr.add(start), end - start)
                    .iter()
                    .map(|byte| *byte as char)
                    .collect(),
            );
            start = end + 1;
        }
        let _ = GlobalUnlock(hglobal);
    }
    out
}

/// Keeps only entries that name an existing local path.
///
/// A clipboard list is untrusted input: a stale `FileNameW` block or one written for a
/// different purpose can hold names that are relative, gone, or not paths at all. Handing
/// those to a copy would fail mid-batch, and a relative name would resolve against the
/// process directory.
fn existing_local(entries: impl IntoIterator<Item = String>) -> Vec<String> {
    entries
        .into_iter()
        .filter(|entry| {
            let path = std::path::Path::new(entry);
            path.is_absolute() && std::fs::symlink_metadata(path).is_ok()
        })
        .map(|entry| native_separators(&entry))
        .collect()
}

/// Reads the clipboard's non-file-list formats, in order of how specific they are.
///
/// A copy made in another application is not required to publish a file list at all:
/// IntelliJ and much of the Java ecosystem put a single `file:` URI in plain text, and the
/// pre-`CF_HDROP` `FileNameW` format is still emitted by some tools. Relying on `CF_HDROP`
/// alone is what made Ctrl+V answer "no files in clipboard" after copying in the IDE.
fn fallback_paths() -> Vec<String> {
    // Registered clipboard formats that carry a path list, and whether the block is UTF-16.
    // The format id is resolved below; the `PCWSTR` spelling itself is never compared,
    // because two identical `w!` literals are different pointers.
    for (name, wide, uri_list) in [
        (w!("FileNameW"), true, false),
        (w!("FileName"), false, false),
        (w!("text/uri-list"), true, true),
    ] {
        let id = unsafe { RegisterClipboardFormatW(name) };
        // An unregistered name is one nobody can be publishing either.
        if id == 0 {
            continue;
        }
        let Some(hglobal) = block(id as u16) else {
            continue;
        };
        let entries = if wide {
            list_from_hglobal(hglobal)
        } else {
            ansi_list(hglobal)
        };
        let paths: Vec<String> = if uri_list {
            parse_uri_list(&entries.join("\r\n"))
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect()
        } else {
            existing_local(entries)
        };
        if !paths.is_empty() {
            return paths;
        }
    }

    // Plain text last: it is the one form every application agrees on, and the shared
    // parser rejects anything that is not a list of existing absolute paths, so a copied
    // snippet of code cannot masquerade as a file transfer.
    if let Some(hglobal) = block(CF_UNICODETEXT.0) {
        if let Some(text) = text_from_hglobal(hglobal) {
            let paths: Vec<String> = parse_text_paths(&text)
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            if !paths.is_empty() {
                return paths;
            }
            // The clipboard holds text, not files. Recording it is what makes "why does
            // paste say there are no files?" answerable from the log alone.
            log::debug!(
                target: "flowdir::clipboard",
                "clipboard text is not a path list: {:?}",
                text.chars().take(120).collect::<String>()
            );
        }
    }
    if let Some(hglobal) = block(CF_TEXT.0) {
        let text = ansi_list(hglobal).join("\r\n");
        let paths: Vec<String> = parse_text_paths(&text)
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        if !paths.is_empty() {
            return paths;
        }
    }
    Vec::new()
}

/// Names of every format the clipboard currently offers, for the diagnostic log below.
///
/// Without this, the only symptom of an unreadable copy is the "no files in clipboard" toast,
/// which says nothing about what the source actually published — the same dead end that made
/// the IntelliJ case unguessable.
fn clipboard_format_names() -> Vec<String> {
    let mut out = Vec::new();
    let mut current = 0u32;
    // `EnumClipboardFormats` returns 0 when the list ends; the cap keeps a misbehaving
    // clipboard client from looping forever on the thread that opened it.
    for _ in 0..256 {
        current = unsafe { EnumClipboardFormats(current) };
        if current == 0 {
            break;
        }
        let mut buffer = [0u16; 128];
        let len = unsafe { GetClipboardFormatNameW(current, &mut buffer) };
        let name = if len > 0 {
            String::from_utf16_lossy(&buffer[..(len as usize).min(buffer.len())])
        } else {
            format!("cf={current}")
        };
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

fn read_locked() -> Clip {
    let cut = read_drop_effect();
    let paths = hdrop_paths();
    if !paths.is_empty() {
        return Clip { paths, cut };
    }
    let paths = fallback_paths();
    if paths.is_empty() {
        // Reaching here is the "paste says there are no files" case. Naming the formats is
        // what turns the next report into a fix instead of another round of guessing.
        log::info!(
            target: "flowdir::clipboard",
            "clipboard holds no readable paths; formats: {}",
            clipboard_format_names().join(", ")
        );
    }
    Clip { paths, cut }
}

/// Reads the clipboard's file list, falling back to its textual forms. A still current
/// in-process WebDAV selection takes precedence because it also carries the remote paths
/// that `CF_HDROP` could not represent.
pub fn read_paths() -> Result<Clip, ApiError> {
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
        let clip = read_locked();
        let _ = CloseClipboard();
        Ok(clip)
    }
}
