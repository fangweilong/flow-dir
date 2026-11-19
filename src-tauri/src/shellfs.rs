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
    SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY,
    CMINVOKECOMMANDINFO,
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

/// Filesystem location behind a shell parsing name, when it has one.
///
/// Aliases such as `shell:Startup` resolve to a real folder, so operations on them must
/// pass the same protection checks as the folder they name. Namespace-only items such as
/// the recycle bin have no filesystem path and return `None`.
pub fn filesystem_path(path: &str) -> Option<String> {
    let _com = ComGuard::new();
    unsafe {
        let mut raw: *mut ITEMIDLIST = std::ptr::null_mut();
        SHParseDisplayName(&HSTRING::from(path), None::<&IBindCtx>, &mut raw, 0, None).ok()?;
        if raw.is_null() {
            return None;
        }
        let pidl = Pidl(raw);
        let item: IShellItem = SHCreateItemFromIDList(pidl.0).ok()?;
        let wide = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let text = wide.to_string().ok().filter(|value| !value.is_empty());
        CoTaskMemFree(Some(wide.0 as *const c_void));
        text
    }
}

/// Whether the path carries the `.lnk` extension, case-insensitively.
pub fn is_shortcut(path: &std::path::Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
}

/// Resolves a `.lnk` shortcut to the filesystem target inside it.
///
/// A shortcut handed to the process from outside — Explorer's "open with", a file dropped
/// onto the executable, a shell `SendTo` entry — is a *file* whose name happens to end in
/// `.lnk`; the path the user meant is the one stored inside it. Without this the caller
/// treats the shortcut itself as the target and shows an empty or failing listing, with no
/// error, because a `.lnk` file relative to its own parent is a perfectly valid path.
pub fn resolve_shortcut(path: &str) -> Option<String> {
    use windows::core::Interface;
    use windows::Win32::System::Com::{
        CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    let _com = ComGuard::new();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let persist: IPersistFile = link.cast().ok()?;
        persist.Load(&HSTRING::from(path), STGM_READ).ok()?;
        let mut buffer = [0u16; 260];
        link.GetPath(&mut buffer, std::ptr::null_mut(), 0).ok()?;
        let len = buffer.iter().position(|unit| *unit == 0)?;
        (len > 0).then(|| String::from_utf16_lossy(&buffer[..len]))
    }
}

/// Whether a parsing name resolves to a shell folder.
///
/// Namespace-only locations (`shell:RecycleBinFolder`, `::{CLSID}`) have no filesystem
/// path, so `Path::is_dir` can never recognize them; the address bar still needs to accept
/// them as navigation targets.
pub fn is_dir(path: &str) -> bool {
    let _com = ComGuard::new();
    unsafe {
        let mut raw: *mut ITEMIDLIST = std::ptr::null_mut();
        if SHParseDisplayName(&HSTRING::from(path), None::<&IBindCtx>, &mut raw, 0, None).is_err()
            || raw.is_null()
        {
            return false;
        }
        let pidl = Pidl(raw);
        let Ok(item) = SHCreateItemFromIDList::<IShellItem>(pidl.0) else {
            return false;
        };
        item.GetAttributes(SFGAO_FOLDER)
            .is_ok_and(|attrs| attrs.0 & SFGAO_FOLDER.0 != 0)
    }
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
            if hr.is_err() {
                // The provider may have handed back an item before failing; it is still
                // ours to release.
                if fetched > 0 && !child.is_null() {
                    CoTaskMemFree(Some(child as *const c_void));
                }
                // An enumeration error leaves unknown items unread; the outcome must not
                // claim the listing is complete.
                truncated = true;
                break;
            }
            if fetched == 0 {
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

    /// Rows carry the shell's own parsing name, which is what every other command resolves
    /// them by. The two shapes seen here are `::{CLSID}\0\::{CLSID}` for namespace items and
    /// a plain `C:\` for drives; both must survive `is_shell_path` classification as written.
    #[test]
    fn parsed_names_are_either_namespace_or_filesystem() {
        assert!(is_shell_path("::{26EE0668-A00A-44D7-9371-BEB064C98683}\\0\\::{025A5937}"));
        assert!(!is_shell_path(r"C:\"));
        assert!(!is_shell_path(r"C:\$Recycle.Bin\S-1-5-21\$R02FNV8"));
    }
}