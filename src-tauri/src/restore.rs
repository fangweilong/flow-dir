//! Restoring files from the Recycle Bin.
//!
//! There is no "restore this path" API: the bin is a shell folder, and each item knows its
//! original folder through the displaced-from property. This walks the bin, matches the
//! requested path case-insensitively and invokes the item's own restore verb, which keeps
//! the behaviour identical to Explorer's (including conflicts with existing files).

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use windows::core::GUID;
use windows::Win32::Foundation::{HWND, PROPERTYKEY};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{
    IContextMenu, IEnumIDList, ILFree, IShellFolder, IShellItem2, SHBindToParent,
    SHCreateItemWithParent, SHGetFolderLocation, CSIDL_BITBUCKET, SHCONTF_FOLDERS,
    SHCONTF_NONFOLDERS, SIGDN_NORMALDISPLAY,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::WindowsAndMessaging::{CreatePopupMenu, DestroyMenu};

use crate::error::ApiError;

/// `PKEY_Recycle_DeletedFrom` is `FMTID_Displaced`/{2}; the shell SDK only defines it for C++.
const PKEY_DISPLACED_FROM: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9b17_4b33_40ff_11d2_a27e_00c0_4fc3_0871),
    pid: 2,
};

#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    pub restored: Vec<String>,
    pub missing: Vec<String>,
}

/// Puts each path back where it came from. Paths that are no longer in the bin are reported
/// instead of failing the whole batch.
pub fn restore(paths: &[String]) -> Result<RestoreReport, ApiError> {
    if paths.is_empty() {
        return Err(ApiError::new("EMPTY", "no paths"));
    }
    // A timed-out shell restore keeps running in its worker thread. Refusing a second
    // call until that thread exits avoids two restores racing over the same bin items.
    if BUSY.swap(true, Ordering::SeqCst) {
        return Err(ApiError::new(
            "RESTORE_BUSY",
            "a previous restore is still running",
        ));
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            BUSY.store(false, Ordering::SeqCst);
        }
    }
    let guard = Guard;
    crate::shell_menu::run_offthread("flowdir-restore", std::time::Duration::from_secs(120), {
        let paths = paths.to_vec();
        move || {
            let _guard = guard;
            unsafe { restore_on_thread(&paths) }
        }
    })?
}

static BUSY: AtomicBool = AtomicBool::new(false);

unsafe fn restore_on_thread(paths: &[String]) -> Result<RestoreReport, ApiError> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| ApiError::new("COM", e.to_string()))?;
        let result = restore_items(paths);
        CoUninitialize();
        result
    }
}

unsafe fn restore_items(paths: &[String]) -> Result<RestoreReport, ApiError> {
    unsafe {
        let wanted: Vec<String> = paths.iter().map(|path| path.to_lowercase()).collect();
        let mut report = RestoreReport::default();
        let folder = recycle_folder()?;
        let mut found: Vec<Option<String>> = vec![None; wanted.len()];
        for_each_recycled(&folder, |item| {
            let Some(original) = original_path(&folder, item) else {
                return true;
            };
            let key = original.to_lowercase();
            if let Some(index) = wanted.iter().position(|path| *path == key) {
                if restore_item(&folder, item).is_ok() {
                    report.restored.push(original);
                    found[index] = Some(key);
                }
            }
            true
        });

        for (index, path) in paths.iter().enumerate() {
            if found[index].is_none() {
                report.missing.push(path.clone());
            }
        }
        Ok(report)
    }
}

/// Visits every item in the bin; the callback returns false to stop early.
unsafe fn for_each_recycled(
    folder: &IShellFolder,
    mut visit: impl FnMut(*const ITEMIDLIST) -> bool,
) {
    unsafe {
        let mut iter: Option<IEnumIDList> = None;
        let _ = folder
            .EnumObjects(
                HWND::default(),
                (SHCONTF_FOLDERS.0 | SHCONTF_NONFOLDERS.0) as u32,
                &mut iter,
            )
            .ok();
        let Some(iter) = iter else {
            return;
        };
        loop {
            let mut ids: [*mut ITEMIDLIST; 1] = [std::ptr::null_mut()];
            let mut fetched = 0u32;
            if iter.Next(&mut ids, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let item = ids[0];
            if item.is_null() {
                break;
            }
            let keep_going = visit(item);
            CoTaskMemFree(Some(item as *const c_void));
            if !keep_going {
                break;
            }
        }
    }
}

/// Original full path of a recycled item. The shell returns the full path as the item's
/// normal display name; older shells return only the name, so the displaced-from folder
/// is the fallback.
unsafe fn original_path(folder: &IShellFolder, item: *const ITEMIDLIST) -> Option<String> {
    unsafe {
        let shell_item: IShellItem2 = SHCreateItemWithParent(None, folder, item).ok()?;
        let name_raw = shell_item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
        let name = name_raw.to_string().ok()?;
        CoTaskMemFree(Some(name_raw.0 as *const c_void));
        if std::path::Path::new(&name).is_absolute() {
            return Some(name);
        }
        let from = shell_item.GetString(&PKEY_DISPLACED_FROM).ok()?;
        let folder_path = from.to_string().ok()?;
        CoTaskMemFree(Some(from.0 as *const c_void));
        Some(join(&folder_path, &name))
    }
}

unsafe fn restore_item(folder: &IShellFolder, item: *const ITEMIDLIST) -> Result<(), ApiError> {
    unsafe {
        let context: IContextMenu = folder
            .GetUIObjectOf(HWND::default(), &[item], None)
            .map_err(|e| ApiError::new("SHELL_MENU", e.to_string()))?;
        let menu = CreatePopupMenu().map_err(|e| ApiError::new("SHELL_MENU", e.to_string()))?;
        let outcome =
            crate::shell_menu::invoke_from_menu(HWND::default(), &context, menu, &["undelete", "restore"]);
        let _ = DestroyMenu(menu);
        outcome
    }
}

unsafe fn recycle_folder() -> Result<IShellFolder, ApiError> {
    unsafe {
        // The known-folder API reports E_FAIL for the virtual bin on some systems; the
        // CSIDL form always yields the desktop-relative item that owns the bin folder.
        let pidl = SHGetFolderLocation(None, CSIDL_BITBUCKET as i32, None, 0)
            .map_err(|e| ApiError::new("SHELL_PATH", e.to_string()))?;
        let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
        let desktop: IShellFolder = SHBindToParent(pidl, Some(&mut child))
            .map_err(|e| ApiError::new("SHELL_BIND", e.to_string()))?;
        let folder: Result<IShellFolder, _> = desktop.BindToObject(child, None::<&IBindCtx>);
        // `child` points into `pidl`; only the full pidl is ours to free.
        ILFree(Some(pidl));
        folder.map_err(|e| ApiError::new("SHELL_FOLDER", e.to_string()))
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('\\') {
        format!("{dir}{name}")
    } else {
        format!("{dir}\\{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `restore` is deliberately single-flight; the tests share that guard.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// End to end: recycle a file through the shell, then restore it by its original path.
    #[test]
    fn restores_a_recycled_file() {
        let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("flowdir-restore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("undo-me.txt");
        std::fs::write(&file, b"payload").unwrap();
        let path = file.to_string_lossy().into_owned();

        assert_eq!(crate::ops::recycle_for_test(&path), 0, "recycle must succeed");
        assert!(!file.exists(), "the file must leave the folder");

        let report = restore(std::slice::from_ref(&path)).expect("restore call");
        assert!(report.missing.is_empty(), "report: {report:?}");
        assert_eq!(report.restored.len(), 1);
        assert!(file.exists(), "the file must be back at its original path");
        assert_eq!(std::fs::read(&file).unwrap(), b"payload");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_path_is_reported_missing() {
        let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let report = restore(&["C:\\flowdir\\no-such-file.txt".to_string()]).expect("restore call");
        assert_eq!(report.missing.len(), 1);
        assert!(report.restored.is_empty());
    }
}
