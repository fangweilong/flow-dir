//! Native Explorer context menu (`IContextMenu`/`2`/`3`).
//!
//! The menu runs on a dedicated STA thread with a hidden owner window: shell
//! extensions (7-Zip, TortoiseGit, …) draw their submenus by handling
//! `WM_INITMENUPOPUP`/`WM_DRAWITEM`/`WM_MEASUREITEM`, so those messages must be
//! forwarded to `IContextMenu2::HandleMenuMsg` or the entries show up blank.

use std::cell::RefCell;
use std::ffi::{c_void, CString};
use std::sync::mpsc;
use std::time::Duration;

use serde::Serialize;
use windows::core::{w, Interface, HSTRING, PCSTR, PCWSTR, PSTR};
use windows::Win32::Foundation::{HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Shell::{
    BHID_SFUIObject, IContextMenu, IContextMenu2, IContextMenu3, IShellFolder,
    SHBindToParent, SHCreateShellItemArrayFromIDLists, SHParseDisplayName, CMF_EXPLORE,
    CMF_EXTENDEDVERBS, CMF_NORMAL, CMIC_MASK_PTINVOKE, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX,
    GCS_VERBW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    GetMenuItemCount, GetMenuItemID, PostMessageW, RegisterClassW, SetForegroundWindow,
    TrackPopupMenuEx, HMENU, MF_SEPARATOR, MF_STRING, SW_SHOWNORMAL, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TPM_WORKAREA, WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR,
    WM_NULL, WNDCLASSW, WS_POPUP,
};

use crate::error::ApiError;

const CMIC_MASK_UNICODE: u32 = 0x0000_4000;
const ID_CMD_FIRST: u32 = 1;
const ID_CMD_LAST: u32 = 0x7FFF;
/// Local ids live above the range handed to `QueryContextMenu`, so the shell never owns them.
const ID_CUSTOM_TOGGLE_HIDDEN: u32 = 0xC000;

#[derive(Serialize, Clone)]
pub struct Outcome {
    pub invoked: bool,
    pub id: u32,
    /// Set when FlowDir's own appended entry was picked; shell commands leave it `None`.
    pub action: Option<String>,
}

thread_local! {
    static CONTEXT: RefCell<Option<(Option<IContextMenu2>, Option<IContextMenu3>)>> = const { RefCell::new(None) };
}

unsafe extern "system" fn owner_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if matches!(
        msg,
        WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR
    ) {
        let handled = CONTEXT.with(|cell| {
            let borrow = cell.borrow();
            let Some((menu2, menu3)) = borrow.as_ref() else {
                return None;
            };
            unsafe {
                if let Some(menu3) = menu3 {
                    let mut result = LRESULT(0);
                    if menu3
                        .HandleMenuMsg2(msg, wparam, lparam, Some(&mut result))
                        .is_ok()
                    {
                        return Some(result);
                    }
                }
                if let Some(menu2) = menu2 {
                    let _ = menu2.HandleMenuMsg(msg, wparam, lparam);
                    return Some(LRESULT(0));
                }
            }
            None
        });
        if let Some(result) = handled {
            return result;
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Shows the shell menu at physical screen coordinates and waits until it closes.
pub fn show(paths: Vec<String>, x: i32, y: i32, extended: bool) -> Result<Outcome, ApiError> {
    if paths.is_empty() {
        return Err(ApiError::new("EMPTY", "no paths"));
    }
    run_on_menu_thread(move || unsafe { run_items(&paths, x, y, extended) })
}

/// Shows Explorer's folder-background menu for `path`. `toggle_label` is appended after the
/// shell's entries because Explorer's own background menu has no hidden-items toggle.
pub fn show_background(
    path: String,
    x: i32,
    y: i32,
    extended: bool,
    toggle_label: String,
) -> Result<Outcome, ApiError> {
    if path.is_empty() {
        return Err(ApiError::new("EMPTY", "no path"));
    }
    run_on_menu_thread(move || unsafe { run_background(&path, x, y, extended, &toggle_label) })
}

/// Runs the menu on its own STA thread: shell extensions expect one and the IPC thread must
/// stay free for the whole time the modal menu is open.
fn run_on_menu_thread(
    work: impl FnOnce() -> Result<Outcome, ApiError> + Send + 'static,
) -> Result<Outcome, ApiError> {
    run_offthread("flowdir-shell-menu", Duration::from_secs(900), work)?
}

/// Runs a blocking shell call off the IPC thread and waits for its result.
pub(crate) fn run_offthread<T: Send + 'static>(
    name: &str,
    timeout: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ApiError> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let _ = tx.send(work());
        })
        .map_err(|e| ApiError::new("THREAD", e.to_string()))?;
    rx.recv_timeout(timeout)
        .map_err(|_| ApiError::new("TIMEOUT", "shell call did not return"))
}

unsafe fn run_items(paths: &[String], x: i32, y: i32, extended: bool) -> Result<Outcome, ApiError> {
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| ApiError::new("COM", e.to_string()))?;
        let result = build_items(paths, x, y, extended);
        CoUninitialize();
        result
    }
}

unsafe fn run_background(
    path: &str,
    x: i32,
    y: i32,
    extended: bool,
    toggle_label: &str,
) -> Result<Outcome, ApiError> {
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| ApiError::new("COM", e.to_string()))?;
        let result = build_background(path, x, y, extended, toggle_label);
        CoUninitialize();
        result
    }
}

unsafe fn build_items(
    paths: &[String],
    x: i32,
    y: i32,
    extended: bool,
) -> Result<Outcome, ApiError> {
    unsafe {
        let mut pidls: Vec<*const windows::Win32::UI::Shell::Common::ITEMIDLIST> =
            Vec::with_capacity(paths.len());
        for path in paths {
            let wide = HSTRING::from(path);
            let mut pidl: *mut windows::Win32::UI::Shell::Common::ITEMIDLIST = std::ptr::null_mut();
            if let Err(err) = SHParseDisplayName(&wide, None::<&IBindCtx>, &mut pidl, 0, None) {
                // The guard was not built yet; free every PIDL parsed so far.
                drop(PidlGuard(pidls));
                return Err(ApiError::from_windows(path, err.code().0));
            }
            pidls.push(pidl as *const _);
        }
        let cleanup = PidlGuard(pidls.clone());

        let array = SHCreateShellItemArrayFromIDLists(&pidls)
            .map_err(|e| ApiError::from_windows("", e.code().0))?;
        let context: IContextMenu = array
            .BindToHandler(None::<&IBindCtx>, &BHID_SFUIObject)
            .map_err(|e| ApiError::new("SHELL_MENU", e.to_string()))?;

        let hwnd = create_owner().map_err(|e| ApiError::new("SHELL_MENU", e.to_string()))?;
        crate::theme::prepare_window(hwnd);
        let flags = CMF_NORMAL | if extended { CMF_EXTENDEDVERBS } else { 0 };
        let outcome = track(hwnd, &context, x, y, flags, None);
        drop(context);
        drop(array);
        drop(cleanup);
        outcome
    }
}

/// Resolves the folder's view object without showing anything; the caller owns `hwnd`, which
/// serves both as the menu owner and as the `CreateViewObject` owner.
unsafe fn background_context(
    path: &str,
    hwnd: HWND,
) -> Result<(IContextMenu, PidlGuard), ApiError> {
    unsafe {
        let wide = HSTRING::from(path);
        let mut pidl: *mut windows::Win32::UI::Shell::Common::ITEMIDLIST = std::ptr::null_mut();
        SHParseDisplayName(&wide, None::<&IBindCtx>, &mut pidl, 0, None)
            .map_err(|e| ApiError::from_windows(path, e.code().0))?;
        let cleanup = PidlGuard(vec![pidl as *const _]);

        // The folder's view object exposes Explorer's blank-area menu; the folder entry's
        // `BHID_SFUIObject` menu would only repeat the row menu.
        let mut child: *mut windows::Win32::UI::Shell::Common::ITEMIDLIST = std::ptr::null_mut();
        let parent: IShellFolder = SHBindToParent(pidl, Some(&mut child))
            .map_err(|e| ApiError::from_windows(path, e.code().0))?;
        if child.is_null() {
            return Err(ApiError::new("SHELL_MENU", "the desktop has no folder background"));
        }
        let folder: IShellFolder = parent
            .BindToObject(child, None::<&IBindCtx>)
            .map_err(|e| ApiError::from_windows(path, e.code().0))?;
        let context: IContextMenu = folder
            .CreateViewObject(hwnd)
            .map_err(|e| ApiError::new("SHELL_MENU", e.to_string()))?;
        Ok((context, cleanup))
    }
}

unsafe fn build_background(
    path: &str,
    x: i32,
    y: i32,
    extended: bool,
    toggle_label: &str,
) -> Result<Outcome, ApiError> {
    unsafe {
        let hwnd = create_owner().map_err(|e| ApiError::new("SHELL_MENU", e.to_string()))?;
        crate::theme::prepare_window(hwnd);
        let (context, cleanup) = match background_context(path, hwnd) {
            Ok(context) => context,
            Err(err) => {
                let _ = DestroyWindow(hwnd);
                return Err(err);
            }
        };
        let flags = CMF_NORMAL | CMF_EXPLORE | if extended { CMF_EXTENDEDVERBS } else { 0 };
        let outcome = track(hwnd, &context, x, y, flags, Some(toggle_label));
        drop(context);
        drop(cleanup);
        outcome
    }
}

/// Builds, tracks and tears down the popup. Takes ownership of `hwnd`.
unsafe fn track(
    hwnd: HWND,
    context: &IContextMenu,
    x: i32,
    y: i32,
    flags: u32,
    toggle_label: Option<&str>,
) -> Result<Outcome, ApiError> {
    unsafe {
        let menu2: Option<IContextMenu2> = context.cast().ok();
        let menu3: Option<IContextMenu3> = context.cast().ok();
        let menu = match CreatePopupMenu() {
            Ok(menu) => menu,
            Err(err) => {
                let _ = DestroyWindow(hwnd);
                return Err(ApiError::new("SHELL_MENU", err.to_string()));
            }
        };

        CONTEXT.with(|cell| *cell.borrow_mut() = Some((menu2, menu3)));

        let query = context.QueryContextMenu(menu, 0, ID_CMD_FIRST, ID_CMD_LAST, flags);
        if query.is_err() {
            CONTEXT.with(|cell| *cell.borrow_mut() = None);
            let _ = DestroyMenu(menu);
            let _ = DestroyWindow(hwnd);
            return Err(ApiError::new("SHELL_MENU", query.message()));
        }

        if let Some(label) = toggle_label {
            let wide = HSTRING::from(label);
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, ID_CUSTOM_TOGGLE_HIDDEN as usize, &wide);
        }

        let _ = SetForegroundWindow(hwnd);
        let command = TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD.0 | TPM_RIGHTBUTTON.0 | TPM_WORKAREA.0,
            x,
            y,
            hwnd,
            None,
        )
        .0 as u32;
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));

        let mut outcome = Outcome {
            invoked: false,
            id: command,
            action: None,
        };
        if command == ID_CUSTOM_TOGGLE_HIDDEN {
            outcome.action = Some("toggleHidden".into());
        } else if (ID_CMD_FIRST..=ID_CMD_LAST).contains(&command) {
            let offset = command - ID_CMD_FIRST;
            let mut info = CMINVOKECOMMANDINFOEX::default();
            info.cbSize = std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32;
            info.fMask = CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE;
            info.hwnd = hwnd;
            info.lpVerb = PCSTR(offset as usize as *const u8);
            info.lpVerbW = PCWSTR(offset as usize as *const u16);
            info.nShow = windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL.0;
            info.ptInvoke = windows::Win32::Foundation::POINT { x, y };
            let invoked = context.InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO);
            if let Err(err) = invoked {
                CONTEXT.with(|cell| *cell.borrow_mut() = None);
                let _ = DestroyMenu(menu);
                let _ = DestroyWindow(hwnd);
                return Err(ApiError::new("SHELL_MENU", err.to_string()));
            }
            outcome.invoked = true;
        }

        CONTEXT.with(|cell| *cell.borrow_mut() = None);
        let _ = DestroyMenu(menu);
        let _ = DestroyWindow(hwnd);
        Ok(outcome)
    }
}

unsafe fn create_owner() -> windows::core::Result<HWND> {
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class_name = w!("FlowDirShellMenuOwner");
        let class = WNDCLASSW {
            lpfnWndProc: Some(owner_proc),
            hInstance: instance,
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&class);
        CreateWindowExW(
            Default::default(),
            class_name,
            w!("FlowDir"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None::<HMENU>,
            Some(instance),
            None,
        )
    }
}

/// Opens the shell property sheet (`Alt+Enter`), independent of the menu path.
pub fn properties(hwnd: Option<isize>, path: &str) -> Result<(), ApiError> {
    use windows::Win32::UI::Shell::{SHObjectProperties, SHOP_FILEPATH};

    let wide = HSTRING::from(path);
    unsafe {
        let owner = hwnd.map(|raw| HWND(raw as *mut c_void));
        let ok = SHObjectProperties(owner, SHOP_FILEPATH, &wide, PCWSTR::null());
        if !ok.as_bool() {
            return Err(ApiError::new(
                "SHELL_FAILED",
                windows::core::Error::from_thread().to_string(),
            ));
        }
    }
    Ok(())
}

/// Invokes a shell verb by name on a drive root; used for `Eject` on removable media.
/// The canonical verb is locale-independent, unlike the menu text.
pub fn eject(path: String) -> Result<(), ApiError> {
    if path.is_empty() {
        return Err(ApiError::new("EMPTY", "no drive"));
    }
    run_offthread("flowdir-eject", Duration::from_secs(30), move || unsafe {
        run_eject(&path)
    })?
}

unsafe fn run_eject(path: &str) -> Result<(), ApiError> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| ApiError::new("COM", e.to_string()))?;
        let result = invoke_verb(path, "Eject");
        CoUninitialize();
        result
    }
}

unsafe fn invoke_verb(path: &str, verb: &str) -> Result<(), ApiError> {
    unsafe {
        let hwnd = create_owner().map_err(|e| ApiError::new("SHELL_MENU", e.to_string()))?;
        let result = invoke_verb_on(hwnd, path, verb);
        let _ = DestroyWindow(hwnd);
        result
    }
}

unsafe fn invoke_verb_on(hwnd: HWND, path: &str, verb: &str) -> Result<(), ApiError> {
    unsafe {
        let wide = HSTRING::from(path);
        let mut pidl: *mut windows::Win32::UI::Shell::Common::ITEMIDLIST = std::ptr::null_mut();
        SHParseDisplayName(&wide, None::<&IBindCtx>, &mut pidl, 0, None)
            .map_err(|e| ApiError::from_windows(path, e.code().0))?;
        let cleanup = PidlGuard(vec![pidl as *const _]);
        let mut child: *mut windows::Win32::UI::Shell::Common::ITEMIDLIST = std::ptr::null_mut();
        let parent: IShellFolder = SHBindToParent(pidl, Some(&mut child))
            .map_err(|e| ApiError::from_windows(path, e.code().0))?;
        if child.is_null() {
            return Err(ApiError::new("SHELL_MENU", "no shell item for the drive"));
        }
        let context: IContextMenu = parent
            .GetUIObjectOf(hwnd, &[child as *const _], None)
            .map_err(|e| ApiError::new("SHELL_MENU", e.to_string()))?;
        let menu = CreatePopupMenu().map_err(|e| ApiError::new("SHELL_MENU", e.to_string()))?;
        let outcome = invoke_from_menu(hwnd, &context, menu, &[verb]);
        let _ = DestroyMenu(menu);
        drop(context);
        drop(cleanup);
        outcome
    }
}

pub(crate) unsafe fn invoke_from_menu(
    hwnd: HWND,
    context: &IContextMenu,
    menu: HMENU,
    verbs: &[&str],
) -> Result<(), ApiError> {
    unsafe {
        // Verb discovery needs a built menu: handlers populate their verbs in QueryContextMenu.
        let query = context.QueryContextMenu(menu, 0, ID_CMD_FIRST, ID_CMD_LAST, CMF_NORMAL);
        if query.is_err() {
            return Err(ApiError::new("SHELL_MENU", "the drive exposes no context menu"));
        }
        let count = GetMenuItemCount(Some(menu));
        let mut found: Option<String> = None;
        for index in 0..count {
            let id = GetMenuItemID(menu, index);
            if !(ID_CMD_FIRST..=ID_CMD_LAST).contains(&id) {
                continue;
            }
            let mut buffer = [0u16; 64];
            if context
                .GetCommandString(
                    (id - ID_CMD_FIRST) as usize,
                    GCS_VERBW,
                    None,
                    PSTR(buffer.as_mut_ptr() as *mut u8),
                    buffer.len() as u32,
                )
                .is_err()
            {
                continue;
            }
            let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
            let found_verb = String::from_utf16_lossy(&buffer[..len]);
            if verbs.iter().any(|verb| found_verb.eq_ignore_ascii_case(verb)) {
                found = Some(found_verb);
                break;
            }
        }
        let Some(verb) = found else {
            return Err(ApiError::new("NO_VERB", format!("none of {verbs:?} for this item")));
        };
        // Both spellings name the verb: the shell accepts a string instead of an offset.
        let ansi = CString::new(verb.as_str()).map_err(|_| ApiError::new("BAD_VERB", &verb))?;
        let wide = HSTRING::from(verb.as_str());
        let info = CMINVOKECOMMANDINFOEX {
            cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
            fMask: CMIC_MASK_UNICODE,
            hwnd,
            lpVerb: PCSTR(ansi.as_ptr() as *const u8),
            lpParameters: PCSTR::null(),
            lpDirectory: PCSTR::null(),
            nShow: SW_SHOWNORMAL.0,
            dwHotKey: 0,
            hIcon: HANDLE::default(),
            lpTitle: PCSTR::null(),
            lpVerbW: PCWSTR(wide.as_ptr()),
            lpParametersW: PCWSTR::null(),
            lpDirectoryW: PCWSTR::null(),
            lpTitleW: PCWSTR::null(),
            ptInvoke: POINT { x: 0, y: 0 },
        };
        context
            .InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO)
            .map_err(|e| ApiError::new("SHELL_MENU", e.to_string()))
    }
}

struct PidlGuard(Vec<*const windows::Win32::UI::Shell::Common::ITEMIDLIST>);

impl Drop for PidlGuard {
    fn drop(&mut self) {
        unsafe {
            for pidl in &self.0 {
                CoTaskMemFree(Some(*pidl as *const c_void));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::GetMenuItemCount;

    /// Guards the integration point: the folder's view object must yield a populated
    /// background menu, otherwise the app silently falls back to the self-drawn menu.
    #[test]
    fn folder_background_exposes_shell_entries() {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                .ok()
                .expect("COM apartment");
            let hwnd = create_owner().expect("owner window");
            let dir = std::env::temp_dir();
            let mut count = -1;
            if let Ok((context, pidls)) = background_context(&dir.to_string_lossy(), hwnd) {
                let menu = CreatePopupMenu().expect("popup menu");
                let flags = CMF_NORMAL | CMF_EXPLORE;
                let query = context.QueryContextMenu(menu, 0, ID_CMD_FIRST, ID_CMD_LAST, flags);
                count = if query.is_ok() {
                    GetMenuItemCount(Some(menu))
                } else {
                    -2
                };
                let _ = DestroyMenu(menu);
                drop(context);
                drop(pidls);
            }
            let _ = DestroyWindow(hwnd);
            CoUninitialize();
            assert!(count > 0, "background menu must contain shell entries, got {count}");
        }
    }
}
