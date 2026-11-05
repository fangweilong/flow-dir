//! Shell-namespace folders (recycle bin, control panel, …) listed like directories.
//!
//! These locations have no filesystem path. Each row carries its desktop-absolute parsing
//! name (`::{CLSID}\…`), which the icon endpoint, the context menu and `list_dir` itself can
//! resolve back through `SHParseDisplayName`.

use std::ffi::c_void;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{GUID, HSTRING, PCSTR, PWSTR};
use windows::Win32::Foundation::{FILETIME, HWND, PROPERTYKEY, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx, COINIT_APARTMENTTHREADED,
    COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::System::SystemServices::{SFGAO_FOLDER, SFGAO_HIDDEN, SFGAO_LINK};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_SFObject, IContextMenu, IShellFolder, IShellItem, IShellItem2, ILCombine,
    SHBindToParent, SHCONTF_FOLDERS, SHCONTF_INCLUDEHIDDEN, SHCONTF_NONFOLDERS,
    SHCreateItemFromIDList, SHGetNameFromIDList, SHParseDisplayName, SIGDN,
    SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY, CMINVOKECOMMANDINFO,
};

use crate::error::ApiError;
use crate::fs::{Entry, ScanOutcome};

const ATTR_DIRECTORY: u32 = 0x10;
const ATTR_HIDDEN: u32 = 0x2;
const ATTR_REPARSE_POINT: u32 = 0x400;
const MAX_ENTRIES: usize = 100_000;

/// `System.Size` (`PKEY_Size`); the crate does not export the property keys.
const PKEY_SIZE: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xb725_f130_47ef_101a_a5f1_0260_8c9e_ebac),
    pid: 12,
};
/// `System.DateModified` (`PKEY_DateModified`).
const PKEY_DATE_MODIFIED: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xb725_f130_47ef_101a_a5f1_0260_8c9e_ebac),
    pid: 14,
};

const WINDOWS_TICKS_PER_MS: u64 = 10_000;
const EPOCH_DIFF_MS: u64 = 11_644_473_600_000;

/// Recycle bin, control panel and other `shell:` / `::` locations the shell can resolve.
pub fn is_shell_path(path: &str) -> bool {
    path.starts_with("shell:") || path.starts_with("::")
}

struct ComGuard(bool);

impl ComGuard {
    fn new() -> Self {
        // Pool threads may already carry an MTA; the folder APIs work in either apartment.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        Self(hr.is_ok() && hr != RPC_E_CHANGED_MODE)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

/// Reads one shell folder. Items keep the order the shell returns them in; `list_dir`
/// sorts afterwards like it does for filesystem listings.
pub fn enumerate(path: &str, show_hidden: bool, cancel: &AtomicBool) -> Result<ScanOutcome, ApiError> {
    let _com = ComGuard::new();
    unsafe {
        let mut raw: *mut ITEMIDLIST = std::ptr::null_mut();
        SHParseDisplayName(&HSTRING::from(path), None::<&IBindCtx>, &mut raw, 0, None)
            .map_err(|e| ApiError::from_windows(path, e.code().0))?;
        if raw.is_null() {
            return Err(ApiError::new("SHELL_FAILED", "no shell item"));
        }
        let root = Pidl(raw);
        // `SHCreateItemFromIDList` requires a fully qualified PIDL, which the parsed root is.
        let item: IShellItem =
            SHCreateItemFromIDList(root.0).map_err(|e| ApiError::from_windows(path, e.code().0))?;
        let folder: IShellFolder = item
            .BindToHandler(None, &BHID_SFObject)
            .map_err(|e| ApiError::from_windows(path, e.code().0))?;

        let mut flags = SHCONTF_FOLDERS.0 | SHCONTF_NONFOLDERS.0;
        if show_hidden {
            flags |= SHCONTF_INCLUDEHIDDEN.0;
        }
        let mut enumerator = None;
        folder
            .EnumObjects(HWND::default(), flags as u32, &mut enumerator)
            .ok()
            .map_err(|e| ApiError::from_windows(path, e.code().0))?;
        let Some(enumerator) = enumerator else {
            return Ok(ScanOutcome {
                entries: Vec::new(),
                truncated: false,
            });
        };

        let mut entries = Vec::new();
        let mut truncated = false;
        loop {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            // `Next` writes through the slice, so the slot must be a real variable; an inline
// array would leave `child` null and every row would describe the folder itself.
            let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
            let mut fetched = 0u32;
            let hr = enumerator.Next(std::slice::from_mut(&mut child), Some(&mut fetched));
            if fetched == 0 || hr.is_err() {
                break;
            }
            let entry = read_item(&folder, root.0, child as *const _, show_hidden);
            // The shell allocated this child PIDL; free it after the row is built.
            CoTaskMemFree(Some(child as *const c_void));
            if let Some(entry) = entry {
                entries.push(entry);
                if entries.len() >= MAX_ENTRIES {
                    truncated = true;
                    break;
                }
            }
        }
        Ok(ScanOutcome { entries, truncated })
    }
}

/// Shell allocations from `ILCombine` / `SHParseDisplayName` must be released with
/// `CoTaskMemFree`, including on the error paths.
struct Pidl(*mut ITEMIDLIST);

impl Drop for Pidl {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0 as *const c_void)) };
    }
}

unsafe fn read_item(
    folder: &IShellFolder,
    root: *const ITEMIDLIST,
    pidl: *const ITEMIDLIST,
    show_hidden: bool,
) -> Option<Entry> {
    unsafe {
        let mut sfgao = SFGAO_FOLDER.0 | SFGAO_HIDDEN.0 | SFGAO_LINK.0;
        folder.GetAttributesOf(&[pidl], &mut sfgao).ok()?;
        let hidden = sfgao & SFGAO_HIDDEN.0 != 0;
        if hidden && !show_hidden {
            return None;
        }
        // The enumerator hands out a PIDL relative to the folder; every name and property
        // API below wants the item's absolute PIDL.
        let absolute = Pidl(ILCombine(Some(root), Some(pidl)));
        if absolute.0.is_null() {
            return None;
        }
        let name = shell_name(absolute.0, SIGDN_NORMALDISPLAY)?;
        let href = shell_name(absolute.0, SIGDN_DESKTOPABSOLUTEPARSING)?;

        let mut attrs = 0u32;
        if sfgao & SFGAO_FOLDER.0 != 0 {
            attrs |= ATTR_DIRECTORY;
        }
        if hidden {
            attrs |= ATTR_HIDDEN;
        }
        if sfgao & SFGAO_LINK.0 != 0 {
            attrs |= ATTR_REPARSE_POINT;
        }

        // Shell locations have no filesystem metadata; the property system is the only source.
        // It is queried per row (the cost is proportional to the listing), so a sky-high
        // recycle bin is slower than Explorer, which parses its `$I` files directly.
        let mut size = 0u64;
        let mut mtime = 0u64;
        if let Ok(item2) = SHCreateItemFromIDList::<IShellItem2>(absolute.0) {
            if let Ok(value) = item2.GetUInt64(&PKEY_SIZE) {
                size = value;
            }
            if let Ok(ft) = item2.GetFileTime(&PKEY_DATE_MODIFIED) {
                mtime = filetime_ms(ft);
            }
        }

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        href.hash(&mut hasher);
        let mut entry = Entry::new(name, size, mtime, attrs, hasher.finish());
        entry.href = Some(href);
        Some(entry)
    }
}

unsafe fn shell_name(pidl: *const ITEMIDLIST, kind: SIGDN) -> Option<String> {
    unsafe {
        let wide: PWSTR = SHGetNameFromIDList(pidl, kind).ok()?;
        let text = wide.to_string().ok();
        CoTaskMemFree(Some(wide.0 as *const c_void));
        text.filter(|value| !value.is_empty())
    }
}

fn filetime_ms(ft: FILETIME) -> u64 {
    let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
    if ticks == 0 {
        return 0;
    }
    (ticks / WINDOWS_TICKS_PER_MS).saturating_sub(EPOCH_DIFF_MS)
}

/// Runs the item's default verb; `ShellExecuteW` cannot resolve `::{CLSID}\…` names.
pub fn open(path: &str) -> Result<(), ApiError> {
    let _com = ComGuard::new();
    unsafe {
        let mut raw: *mut ITEMIDLIST = std::ptr::null_mut();
        SHParseDisplayName(&HSTRING::from(path), None::<&IBindCtx>, &mut raw, 0, None)
            .map_err(|e| ApiError::from_windows(path, e.code().0))?;
        if raw.is_null() {
            return Err(ApiError::new("SHELL_FAILED", "no shell item"));
        }
        let pidl = Pidl(raw);
        let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
        let parent: IShellFolder = SHBindToParent(pidl.0, Some(&mut child))
            .map_err(|e| ApiError::from_windows(path, e.code().0))?;
        if child.is_null() {
            return Err(ApiError::new("SHELL_FAILED", "no shell item"));
        }
        let context: IContextMenu = parent
            .GetUIObjectOf(HWND::default(), &[child as *const _], None)
            .map_err(|e| ApiError::from_windows(path, e.code().0))?;
        // A null verb asks the shell for its default action (open, or "restore" on a
        // recycle-bin item); `SW_SHOWNORMAL` is 1.
        let info = CMINVOKECOMMANDINFO {
            cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
            lpVerb: PCSTR::null(),
            nShow: 1,
            ..Default::default()
        };
        context
            .InvokeCommand(&info)
            .map_err(|e| ApiError::from_windows(path, e.code().0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn control_panel_items_carry_parsing_names() {
        let cancel = AtomicBool::new(false);
        let outcome = enumerate("shell:ControlPanelFolder", true, &cancel).expect("enumerate");
        assert!(!outcome.entries.is_empty());
        assert!(outcome.entries.iter().all(|entry| entry.href.is_some()));
        assert!(outcome.entries.iter().any(|entry| entry.attrs & ATTR_DIRECTORY != 0));
    }

    }