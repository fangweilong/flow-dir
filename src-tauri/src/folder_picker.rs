//! Native folder picker (`IFileOpenDialog` with `FOS_PICKFOLDERS`).
//!
//! The dialog must run on an STA thread with its own message loop, so callers
//! hop onto a dedicated thread instead of the webview/Tauri runtime threads.

use windows::core::HSTRING;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileOpenDialog, IShellItem, FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST,
    FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
};

/// Returns the chosen directory, or `None` when the user cancels.
pub fn pick(owner: HWND, title: &str, initial: &str) -> Option<String> {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        if hr.is_err() {
            return None;
        }
        let result = pick_inner(owner, title, initial);
        CoUninitialize();
        result
    }
}

unsafe fn pick_inner(owner: HWND, title: &str, initial: &str) -> Option<String> {
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL).ok()?;
        let options = dialog.GetOptions().ok()?;
        dialog
            .SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)
            .ok()?;
        if !title.is_empty() {
            dialog.SetTitle(&HSTRING::from(title)).ok()?;
        }
        if !initial.is_empty() {
            if let Ok(folder) = shell_item_from_path(initial) {
                let _ = dialog.SetFolder(&folder);
            }
        }
        if dialog.Show(Some(owner)).is_err() {
            return None;
        }
        let item = dialog.GetResult().ok()?;
        item_path(&item)
    }
}

unsafe fn shell_item_from_path(path: &str) -> windows::core::Result<IShellItem> {
    use windows::Win32::UI::Shell::SHCreateItemFromParsingName;
    unsafe { SHCreateItemFromParsingName(&HSTRING::from(path), None) }
}

unsafe fn item_path(item: &IShellItem) -> Option<String> {
    unsafe {
        let raw = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let text = raw.to_string().ok()?;
        CoTaskMemFree(Some(raw.0 as *const _));
        Some(text)
    }
}
