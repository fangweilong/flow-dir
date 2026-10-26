#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard;
mod dragout;
mod error;
mod folder_picker;
mod fs;
mod git;
mod icons;
mod layout;
mod ops;
mod preview;
mod search;
mod settings;
mod shell_menu;
mod store;
mod theme;
mod titlebar;
mod update;
mod watch;
mod webview_mem;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use error::ApiError;
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{Emitter, Manager};

use windows::core::{w, PCWSTR};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

const BATCH_SIZE: usize = 200;
/// Per-node cap for the sidebar tree.
const TREE_LIMIT: usize = 1000;
/// Records read before sorting: a folder such as `WinSxS` has tens of thousands of
/// subfolders and only the first `TREE_LIMIT` names are ever sent.
const TREE_SCAN_LIMIT: usize = 5_000;

type Scans = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

#[derive(Default)]
struct AppState {
    scans: Scans,
    closing: AtomicBool,
}

impl AppState {
    /// Starts a scan for `tab_id`, cancelling the previous one for the same tab.
    fn begin(&self, tab_id: &str) -> Arc<AtomicBool> {
        let mut map = self.scans.lock().unwrap();
        if let Some(previous) = map.get(tab_id) {
            previous.store(true, Ordering::Relaxed);
        }
        let token = Arc::new(AtomicBool::new(false));
        map.insert(tab_id.to_string(), token.clone());
        token
    }

    fn cancel(&self, tab_id: &str) {
        if let Some(token) = self.scans.lock().unwrap().get(tab_id) {
            token.store(true, Ordering::Relaxed);
        }
    }
}

/// Removes the scan entry on drop, but only if it still belongs to this scan.
struct ScanGuard {
    scans: Scans,
    tab_id: String,
    token: Arc<AtomicBool>,
}

impl Drop for ScanGuard {
    fn drop(&mut self) {
        let mut map = self.scans.lock().unwrap();
        if let Some(current) = map.get(&self.tab_id) {
            if Arc::ptr_eq(current, &self.token) {
                map.remove(&self.tab_id);
            }
        }
    }
}

#[derive(Serialize, Clone)]
struct EntryDto(String, u64, u64, u32, u64);

#[derive(Serialize, Clone)]
struct DirBatch {
    gen: u64,
    scanned: u32,
    done: bool,
    items: Vec<EntryDto>,
    error: Option<ApiError>,
}

#[tauri::command(rename_all = "camelCase")]
fn list_dir(
    state: tauri::State<'_, AppState>,
    path: String,
    tab_id: String,
    gen: u64,
    sort: String,
    dir: String,
    hidden: bool,
    on_batch: Channel<DirBatch>,
) -> Result<(), ApiError> {
    let cancel = state.begin(&tab_id);
    let guard = ScanGuard {
        scans: state.scans.clone(),
        tab_id: tab_id.clone(),
        token: cancel.clone(),
    };

    let _ = tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        let mut entries = match fs::enumerate(&path, hidden, &cancel) {
            Ok(items) => items,
            Err(err) => {
                let _ = on_batch.send(DirBatch {
                    gen,
                    scanned: 0,
                    done: true,
                    items: Vec::new(),
                    error: Some(err),
                });
                return;
            }
        };
        fs::sort_entries(&mut entries, &sort, &dir);

        let mut batch: Vec<EntryDto> = Vec::with_capacity(BATCH_SIZE);
        let mut scanned: u32 = 0;
        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            batch.push(EntryDto(
                entry.name,
                entry.size,
                entry.mtime,
                entry.attrs,
                entry.id,
            ));
            if batch.len() == BATCH_SIZE {
                scanned += batch.len() as u32;
                let items = std::mem::take(&mut batch);
                if on_batch
                    .send(DirBatch {
                        gen,
                        scanned,
                        done: false,
                        items,
                        error: None,
                    })
                    .is_err()
                {
                    return;
                }
                batch = Vec::with_capacity(BATCH_SIZE);
            }
        }
        scanned += batch.len() as u32;
        let _ = on_batch.send(DirBatch {
            gen,
            scanned,
            done: true,
            items: batch,
            error: None,
        });
    });
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
fn cancel_dir(state: tauri::State<'_, AppState>, tab_id: String) {
    state.cancel(&tab_id);
}

/// Relaunches the app; used after a setting that only applies to a fresh WebView2
/// environment (browser arguments such as GPU acceleration).
#[tauri::command]
fn restart_app(app: tauri::AppHandle) {
    app.restart();
}

#[tauri::command]
fn config_path(app: tauri::AppHandle) -> Result<String, ApiError> {
    store::config_dir(&app).map(|dir| dir.to_string_lossy().into_owned())
}

#[tauri::command]
async fn pick_folder(
    window: tauri::WebviewWindow,
    title: String,
    initial: String,
) -> Result<Option<String>, ApiError> {
    let raw = window
        .hwnd()
        .map_err(|e| ApiError::new("WINDOW", e.to_string()))?
        .0 as isize;
    tauri::async_runtime::spawn_blocking(move || {
        let hwnd = windows::Win32::Foundation::HWND(raw as *mut core::ffi::c_void);
        folder_picker::pick(hwnd, &title, &initial)
    })
    .await
    .map_err(|e| ApiError::new("THREAD", e.to_string()))
}

#[tauri::command]
async fn drives() -> Vec<fs::Drive> {
    // Reads `%APPDATA%` and parses shell links, which can sit on a redirected share.
    tauri::async_runtime::spawn_blocking(fs::drives)
        .await
        .unwrap_or_default()
}

/// Repository status for the folder, empty when it is not a work tree or git is absent.
#[tauri::command]
async fn git_status(path: String) -> Result<git::GitStatus, ApiError> {
    tauri::async_runtime::spawn_blocking(move || git::status(&path))
        .await
        .map_err(|e| ApiError::new("GIT_TASK", e.to_string()))
}

/// Subdirectory names of one folder, for the sidebar tree. Capped so a pathological
/// folder cannot flood the panel; the tree only asks for expanded nodes.
#[tauri::command]
async fn tree_dirs(path: String, hidden: bool) -> Result<Vec<String>, ApiError> {
    tauri::async_runtime::spawn_blocking(move || {
        let cancel = AtomicBool::new(false);
        let mut entries = fs::enumerate_dirs(&path, hidden, TREE_SCAN_LIMIT, &cancel)?;
        fs::sort_entries(&mut entries, "name", "asc");
        entries.truncate(TREE_LIMIT);
        Ok(entries.into_iter().map(|entry| entry.name).collect())
    })
    .await
    .map_err(|e| ApiError::new("TREE_TASK", e.to_string()))?
}

/// Whether the path is an existing folder. The address bar checks a target before
/// leaving the current directory, so a typo cannot replace a working folder with an error.
#[tauri::command]
async fn path_is_dir(path: String) -> bool {
    tauri::async_runtime::spawn_blocking(move || std::path::Path::new(&path).is_dir())
        .await
        .unwrap_or(false)
}

#[tauri::command]
fn quick_access() -> Vec<fs::Place> {
    fs::quick_access()
}

#[tauri::command]
fn layout_save(app: tauri::AppHandle, workspace: serde_json::Value) -> Result<(), ApiError> {
    layout::save(&app, workspace)
}

#[tauri::command]
fn layout_load(app: tauri::AppHandle) -> Option<serde_json::Value> {
    layout::load(&app).map(|file| file.workspace)
}

#[tauri::command]
fn layout_preserve(app: tauri::AppHandle) -> Result<String, ApiError> {
    layout::preserve(&app).map(|path| path.display().to_string())
}

#[tauri::command]
fn settings_load(
    app: tauri::AppHandle,
    cache: tauri::State<'_, settings::SettingsCache>,
) -> settings::Settings {
    let settings = settings::load(&app);
    cache.set(settings.clone());
    settings
}

#[tauri::command]
fn settings_save(
    app: tauri::AppHandle,
    cache: tauri::State<'_, settings::SettingsCache>,
    settings: settings::Settings,
) -> Result<(), ApiError> {
    let settings = settings.normalize();
    settings::save(&app, &settings)?;
    cache.set(settings);
    Ok(())
}

#[tauri::command]
fn watch_set(
    app: tauri::AppHandle,
    watchers: tauri::State<'_, watch::Watchers>,
    pane_id: String,
    path: String,
) -> Result<(), ApiError> {
    watch::set(&app, &watchers, pane_id, path)
}

#[tauri::command]
fn watch_clear(watchers: tauri::State<'_, watch::Watchers>, pane_id: String) {
    watch::clear(&watchers, &pane_id);
}

#[tauri::command(rename_all = "camelCase")]
fn clipboard_copy(paths: Vec<String>, cut: bool) -> Result<(), ApiError> {
    clipboard::copy_paths(&paths, cut)
}

#[tauri::command]
fn clipboard_paste() -> Result<clipboard::Clip, ApiError> {
    clipboard::read_paths()
}

/// Starts an OLE drag for the given paths; blocks until the target accepts or cancels.
#[tauri::command(rename_all = "camelCase")]
fn drag_out(paths: Vec<String>) -> Result<String, ApiError> {
    if paths.is_empty() {
        return Err(ApiError::new("EMPTY", "no paths"));
    }
    dragout::drag_out(&paths)
}

#[tauri::command(rename_all = "camelCase")]
fn op_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, ops::OpState>,
    op_id: String,
    kind: String,
    sources: Vec<String>,
    target_dir: Option<String>,
    new_name: Option<String>,
    permanent: bool,
    on_progress: Channel<ops::OpProgress>,
) -> Result<(), ApiError> {
    let hwnd = app
        .get_webview_window("main")
        .and_then(|window| window.hwnd().ok())
        .map(|hwnd| hwnd.0 as isize);
    ops::start(
        &state,
        ops::Request {
            op_id,
            kind,
            sources,
            target_dir,
            new_name,
            permanent,
        },
        hwnd,
        on_progress,
    )
}

#[tauri::command]
fn op_cancel(state: tauri::State<'_, ops::OpState>, op_id: String) {
    state.cancel(&op_id);
}

#[tauri::command]
async fn titlebar_menu(window: tauri::WebviewWindow, x: i32, y: i32) -> Result<(), ApiError> {
    let maximized = window.is_maximized().unwrap_or(false);
    let raw = window
        .hwnd()
        .map_err(|e| ApiError::new("WINDOW", e.to_string()))?
        .0 as isize;
    // The system menu belongs to the UI thread; tracking it elsewhere fails with
    // ERROR_INVALID_PARAMETER. The modal menu loop pumps its own messages, so the
    // main-thread block only lasts while the menu is open.
    window
        .run_on_main_thread(move || {
            let hwnd = windows::Win32::Foundation::HWND(raw as *mut core::ffi::c_void);
            titlebar::show_system_menu(hwnd, maximized, x, y);
        })
        .map_err(|e| ApiError::new("WINDOW", e.to_string()))
}

#[tauri::command]
async fn shell_menu_show(
    paths: Vec<String>,
    x: i32,
    y: i32,
    extended: bool,
) -> Result<shell_menu::Outcome, ApiError> {
    tauri::async_runtime::spawn_blocking(move || shell_menu::show(paths, x, y, extended))
        .await
        .map_err(|e| ApiError::new("THREAD", e.to_string()))?
}

#[tauri::command]
async fn shell_menu_background_show(
    path: String,
    x: i32,
    y: i32,
    extended: bool,
    toggle_label: String,
) -> Result<shell_menu::Outcome, ApiError> {
    tauri::async_runtime::spawn_blocking(move || {
        shell_menu::show_background(path, x, y, extended, toggle_label)
    })
    .await
    .map_err(|e| ApiError::new("THREAD", e.to_string()))?
}

include!(concat!(env!("OUT_DIR"), "/deps.rs"));

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Dependency {
    name: &'static str,
    version: &'static str,
    license: &'static str,
    kind: &'static str,
}

#[derive(Serialize)]
struct AppInfo {
    version: &'static str,
    dependencies: Vec<Dependency>,
}

#[tauri::command(rename_all = "camelCase")]
async fn preview_info(
    app: tauri::AppHandle,
    cache: tauri::State<'_, settings::SettingsCache>,
    path: String,
    width: u32,
    bg: String,
) -> Result<preview::PreviewInfo, ApiError> {
    let bg = u32::from_str_radix(bg.trim_start_matches('#'), 16).unwrap_or(0xffffff);
    // Archive central directories and version resources can sit on a network share:
    // never inspect a file on the IPC thread.
    let external = cache.get(&app).external_preview.clone();
    tauri::async_runtime::spawn_blocking(move || {
        preview::info(path, width.clamp(64, 2048), bg, &external)
    })
    .await
    .map_err(|e| ApiError::new("THREAD", e.to_string()))?
}

#[tauri::command(rename_all = "camelCase")]
fn preview_external(
    app: tauri::AppHandle,
    cache: tauri::State<'_, settings::SettingsCache>,
    path: String,
) -> Result<(), ApiError> {
    let settings = cache.get(&app);
    preview::launch_external(&path, &settings.external_preview)
}

#[tauri::command(rename_all = "camelCase")]
fn search_start(
    state: tauri::State<'_, search::SearchState>,
    root: String,
    query: String,
    gen: u64,
    limit: u32,
    prefer_everything: bool,
    on_batch: Channel<search::SearchBatch>,
) -> Result<(), ApiError> {
    let cancel = state.begin();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        search::run(root, query, gen, limit, prefer_everything, cancel, on_batch);
    });
    Ok(())
}

#[tauri::command]
fn set_app_theme(app: tauri::AppHandle, theme: String) {
    let hwnd = app
        .get_webview_window("main")
        .and_then(|window| window.hwnd().ok())
        .map(|hwnd| windows::Win32::Foundation::HWND(hwnd.0));
    theme::apply(hwnd, &theme);
}

#[tauri::command]
fn everything_status() -> search::EverythingStatus {
    search::everything_status()
}

#[tauri::command]
fn shell_properties(app: tauri::AppHandle, path: String) -> Result<(), ApiError> {
    let hwnd = app
        .get_webview_window("main")
        .and_then(|window| window.hwnd().ok());
    shell_menu::properties(hwnd.map(|h| h.0 as isize), &path)
}

/// Invokes the shell's `Eject` verb; media removal itself is asynchronous.
#[tauri::command]
fn drive_eject(root: String) -> Result<(), ApiError> {
    shell_menu::eject(root)
}

#[tauri::command]
fn search_cancel(state: tauri::State<'_, search::SearchState>) {
    state.cancel();
}

#[tauri::command]
async fn update_check(
    app: tauri::AppHandle,
    force: bool,
) -> Result<update::UpdateStatus, ApiError> {
    tauri::async_runtime::spawn_blocking(move || update::check(&app, force))
        .await
        .map_err(|e| ApiError::new("UPDATE_TASK", e.to_string()))
}

#[tauri::command]
async fn update_install(app: tauri::AppHandle) -> Result<(), ApiError> {
    tauri::async_runtime::spawn_blocking(move || update::install(&app))
        .await
        .map_err(|e| ApiError::new("UPDATE_TASK", e.to_string()))?
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        dependencies: DEPS
            .iter()
            .map(|(kind, name, version, license)| Dependency {
                kind,
                name,
                version,
                license,
            })
            .collect(),
    }
}

#[tauri::command]
fn open_path(path: String) -> Result<(), ApiError> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if (result.0 as isize) <= 32 {
        return Err(ApiError::new(
            "SHELL_FAILED",
            format!("ShellExecuteW 返回 {}", result.0 as isize),
        ));
    }
    Ok(())
}

#[tauri::command]
fn explorer_open(path: String) -> Result<(), ApiError> {
    if !std::path::Path::new(&path).is_dir() {
        return Err(ApiError::new("NOT_DIR", path));
    }
    std::process::Command::new("explorer")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| ApiError::from_io(&path, &e))
}

#[tauri::command]
fn reveal_path(path: String) -> Result<(), ApiError> {
    std::process::Command::new("explorer")
        .arg("/select,")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| ApiError::from_io(&path, &e))
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn init_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let show = MenuItem::with_id(app, "show", "显示窗口 / Show", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出 / Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().unwrap())
        .tooltip(store::display_name(app))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "quit" => app.exit(0),
            "show" => show_main(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main(app);
        }))
        .manage(AppState::default())
        .manage(ops::OpState::default())
        .manage(watch::Watchers::default())
        .manage(preview::PreviewCache::default())
        .manage(settings::SettingsCache::default())
        .manage(search::SearchState::default())
        .register_asynchronous_uri_scheme_protocol("fd", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let uri = request.uri().clone();
            tauri::async_runtime::spawn_blocking(move || {
                let response = if uri.path() == "/icon" {
                    icons::handle_request(&app, &uri)
                } else {
                    preview::handle_request(&app, &uri)
                };
                responder.respond(response);
            });
        })
        .setup(|app| {
            store::migrate_legacy(app.handle());
            update::cleanup_leftovers();
            let saved = settings::load(app.handle());
            app.state::<settings::SettingsCache>().set(saved.clone());
            // The window is built here instead of in tauri.conf.json because the WebView2
            // browser arguments (GPU on/off) are fixed when the browser process starts.
            let window = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App(format!("index.html?v={}", env!("CARGO_PKG_VERSION")).into()),
            )
            .title(store::display_name(app.handle()))
            .inner_size(1280.0, 820.0)
            .min_inner_size(900.0, 520.0)
            .center()
            .decorations(false)
            .visible(false)
            .additional_browser_args(&webview_mem::browser_args(saved.gpu_accel))
            .data_directory(
                store::cache_dir(app.handle())
                    .map(|dir| dir.join("EBWebView"))
                    .unwrap_or_else(|_| std::path::PathBuf::from("EBWebView")),
            )
            // Release builds must not expose a web inspector, whatever the WebView2 default is.
            .devtools(cfg!(debug_assertions))
            .build()?;
            layout::restore_window(app.handle());
            let hwnd = app
                .get_webview_window("main")
                .and_then(|window| window.hwnd().ok())
                .map(|hwnd| windows::Win32::Foundation::HWND(hwnd.0));
            theme::apply(hwnd, &saved.theme);
            titlebar::apply(&window);
            webview_mem::watch(app.handle().clone());
            update::spawn_scheduler(app.handle().clone());
            let _ = window.show();
            let _ = window.set_focus();
            if let Err(err) = init_tray(app.handle()) {
                eprintln!("tray init failed: {err}");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Resized(_) = event {
                if let Ok(hwnd) = window.hwnd() {
                    let hwnd = windows::Win32::Foundation::HWND(hwnd.0);
                    let maximized = window.is_maximized().unwrap_or(false);
                    titlebar::set_shadow(hwnd, !maximized);
                }
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<AppState>();
                if !state.closing.swap(true, Ordering::SeqCst) {
                    api.prevent_close();
                    let _ = window.emit("flowdir:flush-layout", ());
                    let win = window.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(900));
                        let _ = win.close();
                    });
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_dir,
            cancel_dir,
            drives,
            tree_dirs,
            path_is_dir,
            git_status,
            quick_access,
            open_path,
            explorer_open,
            reveal_path,
            layout_save,
            layout_preserve,
            layout_load,
            settings_load,
            settings_save,
            config_path,
            restart_app,
            pick_folder,
            set_app_theme,
            watch_set,
            watch_clear,
            clipboard_copy,
            clipboard_paste,
            drag_out,
            op_start,
            op_cancel,
            shell_menu_show,
            shell_menu_background_show,
            titlebar_menu,
            preview_info,
            preview_external,
            search_start,
            search_cancel,
            everything_status,
            shell_properties,
            drive_eject,
            app_info,
            update_check,
            update_install
        ])
        .run(tauri::generate_context!())
        .expect("FlowDir failed to start");
}
