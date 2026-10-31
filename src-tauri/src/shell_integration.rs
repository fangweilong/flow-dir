//! User-level Windows integration: global hotkey, Explorer context menu, "open with"
//! registration for folders and drives, autostart and command-line launch requests.
//!
//! Everything here writes only under `HKCU` and is reversible: turning a switch off deletes
//! the keys this module created. The folder association never claims Explorer's default
//! verb; it only adds a ProgID to `OpenWithProgids`, so the system default stays untouched.
//! Development builds use their own key namespace (`FlowDirDev`), so they cannot overwrite
//! the release integration.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LPARAM, WPARAM, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS, REG_SZ,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    MOD_SHIFT, MOD_WIN, VIRTUAL_KEY, VK_BACK, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME,
    VK_INSERT, VK_LEFT, VK_NEXT, VK_OEM_COMMA, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB,
    VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PostThreadMessageW, TranslateMessage, MSG, WM_HOTKEY, WM_QUIT,
};

use crate::error::ApiError;
use crate::settings::Settings;
use crate::store;

const HOTKEY_ID: i32 = 0xA11;
const MAX_RECENT: usize = 10;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const OPEN_WITH_PROGIDS: [&str; 2] = [
    r"Software\Classes\Directory\OpenWithProgids",
    r"Software\Classes\Drive\OpenWithProgids",
];

/// What a launch (or a second instance) asks the frontend to open.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRequest {
    /// Folder to show; a file passed on the command line is split into its parent.
    pub path: Option<String>,
    /// Row to select inside `path`.
    pub select: Option<String>,
    /// Open in a new tab instead of replacing the current one.
    pub new_window: bool,
    /// Started by the HKCU Run entry; the window may stay hidden.
    pub autostart: bool,
}

impl LaunchRequest {
    /// A bare second launch carries nothing to open.
    pub fn is_empty(&self) -> bool {
        self.path.is_none() && !self.new_window
    }
}

struct Hotkey {
    combo: String,
    thread_id: u32,
    join: std::thread::JoinHandle<()>,
}

/// Process-wide integration state; the hotkey thread owns the message queue, so the
/// handle must outlive the settings that started it.
#[derive(Default)]
pub struct Integration {
    hotkey: Mutex<Option<Hotkey>>,
    recent: Mutex<Vec<String>>,
    launch: Mutex<Option<LaunchRequest>>,
}

impl Integration {
    pub fn set_launch(&self, request: LaunchRequest) {
        *self.launch.lock().unwrap() = Some(request);
    }

    pub fn take_launch(&self) -> Option<LaunchRequest> {
        self.launch.lock().unwrap().take()
    }
}

/// `FlowDir` or `FlowDirDev`; the same suffix keeps build variants from sharing keys.
fn namespace(app: &AppHandle) -> &'static str {
    if store::is_dev(app) {
        "FlowDirDev"
    } else {
        "FlowDir"
    }
}

fn display(app: &AppHandle) -> &'static str {
    store::display_name(app)
}

/// Applies every integration switch that is currently on and removes whatever it turned
/// off. Returns stable codes (`HOTKEY_CONFLICT`, `HOTKEY_INVALID`, `INTEGRATION_FAILED`)
/// so the frontend can localize the toast.
pub fn apply(app: &AppHandle, settings: &Settings) -> Vec<String> {
    let mut warnings: Vec<String> = Vec::new();
    let ns = namespace(app);
    let exe = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_default();

    if exe.is_empty() {
        warnings.push("INTEGRATION_FAILED".into());
    } else {
        let label = display(app);
        let command = format!("\"{exe}\" \"%1\"");

        // Explorer context menu for files and folders.
        let file_menu = format!(r"Software\Classes\*\shell\{ns}");
        let dir_menu = format!(r"Software\Classes\Directory\shell\{ns}");
        if settings.context_menu {
            for key in [&file_menu, &dir_menu] {
                let result = write_string(HKEY_CURRENT_USER, key, None, label)
                    .and_then(|_| write_string(HKEY_CURRENT_USER, key, Some("Icon"), &format!("\"{exe}\",0")))
                    .and_then(|_| {
                        write_string(
                            HKEY_CURRENT_USER,
                            &format!(r"{key}\command"),
                            None,
                            &command,
                        )
                    });
                if result.is_err() {
                    push_once(&mut warnings, "INTEGRATION_FAILED");
                }
            }
        } else {
            delete_tree(HKEY_CURRENT_USER, &file_menu);
            delete_tree(HKEY_CURRENT_USER, &dir_menu);
        }

        // Folder/drive association: an "Open with" ProgID, never the default verb.
        let progid = format!("{ns}.Folder");
        if settings.associate_folders {
            let prog_root = format!(r"Software\Classes\{progid}");
            let result = write_string(HKEY_CURRENT_USER, &prog_root, None, label)
                .and_then(|_| {
                    write_string(
                        HKEY_CURRENT_USER,
                        &format!(r"{prog_root}\DefaultIcon"),
                        None,
                        &format!("\"{exe}\",0"),
                    )
                })
                .and_then(|_| {
                    write_string(
                        HKEY_CURRENT_USER,
                        &format!(r"{prog_root}\shell\open"),
                        None,
                        "Open",
                    )
                })
                .and_then(|_| {
                    write_string(
                        HKEY_CURRENT_USER,
                        &format!(r"{prog_root}\shell\open\command"),
                        None,
                        &command,
                    )
                });
            for key in OPEN_WITH_PROGIDS {
                if result.is_err() {
                    break;
                }
                if write_string(HKEY_CURRENT_USER, key, Some(&progid), "").is_err() {
                    push_once(&mut warnings, "INTEGRATION_FAILED");
                }
            }
            if result.is_err() {
                push_once(&mut warnings, "INTEGRATION_FAILED");
            }
        } else {
            delete_tree(HKEY_CURRENT_USER, &format!(r"Software\Classes\{progid}"));
            for key in OPEN_WITH_PROGIDS {
                delete_value(HKEY_CURRENT_USER, key, &progid);
            }
        }

        if settings.autostart {
            let value = format!("\"{exe}\" --autostart");
            if write_string(HKEY_CURRENT_USER, RUN_KEY, Some(ns), &value).is_err() {
                push_once(&mut warnings, "INTEGRATION_FAILED");
            }
        } else {
            delete_value(HKEY_CURRENT_USER, RUN_KEY, ns);
        }
    }

    if let Err(code) = apply_hotkey(app, settings.hotkey_enabled, &settings.global_hotkey) {
        push_once(&mut warnings, &code);
    }
    warnings
}

fn push_once(warnings: &mut Vec<String>, code: &str) {
    if !warnings.iter().any(|item| item == code) {
        warnings.push(code.to_string());
    }
}

// ---------------------------------------------------------------- hotkey

/// Registers (or clears) the global hotkey. Re-applying the same combination is a no-op so
/// the debounced settings save does not restart the message-loop thread on every change.
fn apply_hotkey(app: &AppHandle, enabled: bool, combo: &str) -> Result<(), String> {
    let desired = if enabled {
        Some(canonical_hotkey(combo).ok_or_else(|| "HOTKEY_INVALID".to_string())?)
    } else {
        None
    };

    let state = app.state::<Integration>();
    let mut slot = state.hotkey.lock().unwrap();
    if slot.as_ref().map(|handle| handle.combo.as_str()) == desired.as_deref() {
        return Ok(());
    }
    if let Some(handle) = slot.take() {
        stop_hotkey(handle);
    }
    let Some(combo) = desired else {
        return Ok(());
    };
    let (modifiers, vk) = parse_hotkey(&combo).ok_or_else(|| "HOTKEY_INVALID".to_string())?;

    let (tx, rx) = mpsc::channel();
    let thread_app = app.clone();
    let thread = std::thread::Builder::new()
        .name("flowdir-hotkey".into())
        .spawn(move || {
            let thread_id = unsafe { GetCurrentThreadId() };
            match unsafe { RegisterHotKey(None, HOTKEY_ID, modifiers, vk) } {
                Ok(()) => {
                    let _ = tx.send(Ok(thread_id));
                    hotkey_loop(&thread_app);
                    let _ = unsafe { UnregisterHotKey(None, HOTKEY_ID) };
                }
                Err(err) => {
                    let _ = tx.send(Err(err.message()));
                }
            }
        })
        .map_err(|err| err.to_string())?;

    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(thread_id)) => {
            *slot = Some(Hotkey {
                combo,
                thread_id,
                join: thread,
            });
            Ok(())
        }
        Ok(Err(_)) => {
            let _ = thread.join();
            Err("HOTKEY_CONFLICT".to_string())
        }
        // The thread owns a message loop from here on; joining could block forever, so the
        // handle is dropped and the (extremely unlikely) stray hotkey stays until exit.
        Err(_) => Err("HOTKEY_CONFLICT".to_string()),
    }
}

fn stop_hotkey(handle: Hotkey) {
    // WM_QUIT makes GetMessageW return 0 and the thread unregisters on the way out.
    let _ = unsafe { PostThreadMessageW(handle.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
    let _ = handle.join.join();
}

fn hotkey_loop(app: &AppHandle) {
    let mut message = MSG::default();
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            if message.message == WM_HOTKEY && message.wParam.0 == HOTKEY_ID as usize {
                crate::show_main(app);
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

/// `ctrl+alt+f` → flags plus virtual key. Requires at least one modifier: a bare key would
/// be swallowed system-wide.
pub fn parse_hotkey(combo: &str) -> Option<(HOT_KEY_MODIFIERS, u32)> {
    let mut modifiers = MOD_NOREPEAT;
    let mut key: Option<u32> = None;
    for token in combo
        .split('+')
        .map(|part| part.trim().to_ascii_lowercase())
        .filter(|part| !part.is_empty())
    {
        match token.as_str() {
            "ctrl" | "control" | "meta" => modifiers |= MOD_CONTROL,
            "alt" => modifiers |= MOD_ALT,
            "shift" => modifiers |= MOD_SHIFT,
            "win" => modifiers |= MOD_WIN,
            other => {
                if key.is_some() {
                    return None;
                }
                key = Some(virtual_key(other)?);
            }
        }
    }
    let bare = modifiers.0 & (MOD_CONTROL.0 | MOD_ALT.0 | MOD_SHIFT.0 | MOD_WIN.0) == 0;
    if bare {
        return None;
    }
    Some((modifiers, key?))
}

/// Canonical spelling used for change detection, so `Ctrl+Alt+F` and `ctrl+alt+f` match.
fn canonical_hotkey(combo: &str) -> Option<String> {
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut win = false;
    let mut key: Option<String> = None;
    for token in combo
        .split('+')
        .map(|part| part.trim().to_ascii_lowercase())
        .filter(|part| !part.is_empty())
    {
        match token.as_str() {
            "ctrl" | "control" | "meta" => ctrl = true,
            "alt" => alt = true,
            "shift" => shift = true,
            "win" => win = true,
            other => {
                if key.is_some() {
                    return None;
                }
                virtual_key(other)?;
                key = Some(other.to_string());
            }
        }
    }
    let key = key?;
    if !(ctrl || alt || shift || win) {
        return None;
    }
    let mut parts = Vec::new();
    if ctrl {
        parts.push("ctrl".to_string());
    }
    if alt {
        parts.push("alt".to_string());
    }
    if shift {
        parts.push("shift".to_string());
    }
    if win {
        parts.push("win".to_string());
    }
    parts.push(key);
    Some(parts.join("+"))
}

fn virtual_key(token: &str) -> Option<u32> {
    let named = match token {
        "space" => VK_SPACE,
        "tab" => VK_TAB,
        "enter" | "return" => VK_RETURN,
        "esc" | "escape" => VK_ESCAPE,
        "delete" | "del" => VK_DELETE,
        "backspace" | "bksp" => VK_BACK,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        "home" => VK_HOME,
        "end" => VK_END,
        "insert" | "ins" => VK_INSERT,
        "pgup" | "pageup" => VK_PRIOR,
        "pgdn" | "pagedown" => VK_NEXT,
        "comma" | "," => VK_OEM_COMMA,
        _ => VIRTUAL_KEY(0),
    };
    if named.0 != 0 {
        return Some(named.0 as u32);
    }
    if let Some(digit) = token.chars().next().filter(|_| token.len() == 1) {
        if digit.is_ascii_alphanumeric() {
            return Some(digit.to_ascii_uppercase() as u32);
        }
    }
    if let Some(rest) = token.strip_prefix('f') {
        if let Ok(number) = rest.parse::<u32>() {
            if (1..=24).contains(&number) {
                return Some(0x70 + number - 1);
            }
        }
    }
    None
}

// ---------------------------------------------------------------- command line

/// Parses the arguments after the executable. Paths may be relative; `--select` names a
/// file and is split into its parent folder plus the row to select.
pub fn parse_launch<I: IntoIterator<Item = String>>(args: I) -> LaunchRequest {
    let mut request = LaunchRequest::default();
    let mut path_arg: Option<String> = None;
    let mut select_arg: Option<String> = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == "--autostart" {
            request.autostart = true;
        } else if arg == "--new-window" {
            request.new_window = true;
        } else if let Some(value) = arg.strip_prefix("--select=") {
            select_arg = Some(value.to_string());
        } else if arg == "--select" {
            if let Some(value) = iter.next() {
                select_arg = Some(value);
            }
        } else if arg.starts_with("--") {
            // Unknown switches are ignored rather than treated as a path.
        } else if path_arg.is_none() {
            path_arg = Some(arg);
        }
    }

    if let Some(path) = path_arg {
        let (dir, file) = split_target(&path, false);
        request.path = Some(dir);
        if file.is_some() {
            request.select = file;
        }
    }
    if let Some(select) = select_arg {
        if request.path.is_some() && !select.contains(['\\', '/']) {
            // `flowdir D:\dir --select report.txt`
            request.select = Some(select);
        } else {
            let (dir, file) = split_target(&select, true);
            if request.path.is_none() {
                request.path = Some(dir);
            }
            request.select = file;
        }
    }
    request
}

/// Splits a command-line target into a folder and an optional row name. `as_file` treats
/// the input as a file even when it does not exist yet (`--select`).
fn split_target(path: &str, as_file: bool) -> (String, Option<String>) {
    let mut target = PathBuf::from(path);
    if !target.is_absolute() {
        if let Ok(cwd) = std::env::current_dir() {
            target = cwd.join(target);
        }
    }
    if !as_file && (target.is_dir() || (!target.exists() && target.extension().is_none())) {
        return (target.display().to_string(), None);
    }
    match (target.parent(), target.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() && !name.is_empty() => (
            parent.display().to_string(),
            Some(name.to_string_lossy().into_owned()),
        ),
        _ => (target.display().to_string(), None),
    }
}

// ---------------------------------------------------------------- tray

/// Tray menu shared by the initial build and the recent-directory rebuild.
pub fn tray_menu(app: &AppHandle, recent: &[String]) -> tauri::Result<Menu<tauri::Wry>> {
    let show = MenuItem::with_id(app, "show", "显示窗口 / Show", true, None::<&str>)?;
    let search = MenuItem::with_id(app, "search", "快速搜索 / Search", true, None::<&str>)?;
    let mut entries = Vec::new();
    if !recent.is_empty() {
        for (index, dir) in recent.iter().enumerate().take(MAX_RECENT) {
            // A literal `&` would become a mnemonic and lose its underline target.
            entries.push(MenuItem::with_id(
                app,
                format!("recent:{index}"),
                dir.replace('&', "&&"),
                true,
                None::<&str>,
            )?);
        }
    }
    let recent_menu = if entries.is_empty() {
        None
    } else {
        let refs: Vec<&dyn IsMenuItem<tauri::Wry>> = entries
            .iter()
            .map(|item| item as &dyn IsMenuItem<tauri::Wry>)
            .collect();
        Some(Submenu::with_items(
            app,
            "最近目录 / Recent",
            true,
            &refs,
        )?)
    };
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出 / Quit", true, None::<&str>)?;

    let mut items: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![&show, &search];
    if let Some(menu) = &recent_menu {
        items.push(menu);
    }
    items.push(&separator);
    items.push(&quit);
    Menu::with_items(app, &items)
}

/// Rebuilds the tray menu from the latest recent-directory list.
pub fn set_recent(app: &AppHandle, dirs: Vec<String>) -> Result<(), ApiError> {
    let dirs: Vec<String> = dirs
        .into_iter()
        .map(|dir| dir.trim().to_string())
        .filter(|dir| !dir.is_empty())
        .take(MAX_RECENT)
        .collect();
    *app.state::<Integration>().recent.lock().unwrap() = dirs.clone();
    if let Some(tray) = app.tray_by_id("main") {
        let menu = tray_menu(app, &dirs)
            .map_err(|err| ApiError::new("TRAY_MENU", err.to_string()))?;
        tray.set_menu(Some(menu))
            .map_err(|err| ApiError::new("TRAY_MENU", err.to_string()))?;
    }
    Ok(())
}

/// Menu ids other than `quit`, which needs the busy-operation confirmation in `main`.
pub fn on_menu_event(app: &AppHandle, id: &str) {
    if id == "show" {
        crate::show_main(app);
        return;
    }
    if id == "search" {
        crate::show_main(app);
        let _ = app.emit("flowdir:search", ());
        return;
    }
    let Some(index) = id.strip_prefix("recent:").and_then(|rest| rest.parse::<usize>().ok()) else {
        return;
    };
    let path = app.state::<Integration>().recent.lock().unwrap().get(index).cloned();
    if let Some(path) = path {
        crate::show_main(app);
        let request = LaunchRequest {
            path: Some(path),
            select: None,
            new_window: true,
            autostart: false,
        };
        let _ = app.emit("flowdir:open-request", request);
    }
}

// ---------------------------------------------------------------- registry

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn create_key(root: HKEY, subkey: &str, access: REG_SAM_FLAGS) -> Result<HKEY, String> {
    let subkey = wide(subkey);
    let mut key = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            root,
            PCWSTR(subkey.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            access,
            None,
            &mut key,
            None,
        )
    };
    if status == WIN32_ERROR(0) {
        Ok(key)
    } else {
        Err(format!("RegCreateKeyExW: {}", status.0))
    }
}

fn write_string(
    root: HKEY,
    subkey: &str,
    name: Option<&str>,
    value: &str,
) -> Result<(), String> {
    let key = create_key(root, subkey, KEY_WRITE)?;
    let name_wide = name.map(wide);
    let name_ptr = name_wide
        .as_ref()
        .map(|wide| PCWSTR(wide.as_ptr()))
        .unwrap_or(PCWSTR::null());
    let data = wide(value);
    let bytes = unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), data.len() * 2) };
    let status =
        unsafe { RegSetValueExW(key, name_ptr, None, REG_SZ, Some(bytes)) };
    let _ = unsafe { RegCloseKey(key) };
    if status == WIN32_ERROR(0) {
        Ok(())
    } else {
        Err(format!("RegSetValueExW: {}", status.0))
    }
}

fn delete_tree(root: HKEY, subkey: &str) {
    let subkey = wide(subkey);
    let _ = unsafe { RegDeleteTreeW(root, PCWSTR(subkey.as_ptr())) };
}

fn delete_value(root: HKEY, subkey: &str, name: &str) {
    let subkey = wide(subkey);
    let mut key = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            root,
            PCWSTR(subkey.as_ptr()),
            None,
            KEY_WRITE,
            &mut key,
        )
    };
    if status != WIN32_ERROR(0) {
        return;
    }
    let name = wide(name);
    let _ = unsafe { RegDeleteValueW(key, PCWSTR(name.as_ptr())) };
    let _ = unsafe { RegCloseKey(key) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_parses_modifiers_and_keys() {
        let (modifiers, vk) = parse_hotkey("ctrl+alt+f").unwrap();
        assert_eq!(
            modifiers.0 & (MOD_CONTROL.0 | MOD_ALT.0),
            MOD_CONTROL.0 | MOD_ALT.0
        );
        assert_eq!(vk, 'F' as u32);
        assert_eq!(parse_hotkey("ctrl+shift+f12").unwrap().1, 0x7B);
        assert_eq!(parse_hotkey("alt+space").unwrap().1, VK_SPACE.0 as u32);
        // A bare key would be grabbed system-wide.
        assert!(parse_hotkey("f").is_none());
        assert!(parse_hotkey("ctrl+alt+").is_none());
        assert!(parse_hotkey("ctrl+nope").is_none());
    }

    #[test]
    fn hotkey_canonicalizes_case_and_order() {
        assert_eq!(canonical_hotkey(" Ctrl + Alt + F ").as_deref(), Some("ctrl+alt+f"));
        assert_eq!(canonical_hotkey("alt+ctrl+f").as_deref(), Some("ctrl+alt+f"));
        assert!(canonical_hotkey("ctrl+alt").is_none());
    }

    #[test]
    fn launch_flags_and_unknown_switches() {
        let request = parse_launch(
            ["--new-window", "--verbose", "--autostart"]
                .iter()
                .map(|value| value.to_string()),
        );
        assert!(request.new_window);
        assert!(request.autostart);
        assert!(request.path.is_none());
        assert!(!request.is_empty());

        let bare = parse_launch(["--verbose"].iter().map(|value| value.to_string()));
        assert!(bare.is_empty());
    }

    /// A `--select` value that ends in a file name is split even when it does not exist.
    #[test]
    fn launch_select_splits_into_folder_and_row() {
        let request = parse_launch(
            ["--select", r"D:\folder\missing.txt"]
                .iter()
                .map(|value| value.to_string()),
        );
        assert_eq!(request.path.as_deref(), Some(r"D:\folder"));
        assert_eq!(request.select.as_deref(), Some("missing.txt"));
        assert!(!request.is_empty());
    }
}