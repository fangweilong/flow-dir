#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod archive;
mod clipboard;
mod compare;
mod dragout;
mod elevated;
mod error;
mod folder_picker;
mod fs;
mod git;
mod hash;
mod dup;
mod icons;
mod layout;
mod logging;
mod naming;
mod ops;
mod preview;
mod restore;
mod search;
mod settings;
mod shell_integration;
mod shell_menu;
mod store;
mod taskbar;
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
    /// True on the final batch when the entry cap cut the listing short.
    truncated: bool,
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
        let started = std::time::Instant::now();
        log::info!(
            target: "flowdir::fs",
            "list_dir start tab={tab_id} sort={sort}/{dir} hidden={hidden}"
        );
        // Full paths are opt-in: enable the debug level when a report needs them.
        log::debug!(target: "flowdir::fs", "list_dir path={path}");
        // Progress-only batches go out while the directory is still being read, so a slow
        // share grows the counter instead of leaving the pane on a spinner.
        let mut reported = 0usize;
        let outcome = fs::enumerate_reporting(&path, hidden, &cancel, &mut |count| {
            if count < reported + 256 {
                return;
            }
            reported = count;
            let _ = on_batch.send(DirBatch {
                gen,
                scanned: count as u32,
                done: false,
                truncated: false,
                items: Vec::new(),
                error: None,
            });
        });
        let (mut entries, truncated) = match outcome {
            Ok(outcome) => (outcome.entries, outcome.truncated),
            Err(err) => {
                logging::error_once(
                    &format!("list_dir:{}:{}", err.code, path),
                    &format!("list_dir failed code={}", err.code),
                );
                log::debug!(
                    target: "flowdir::fs",
                    "list_dir error path={path}: {} {}",
                    err.code,
                    err.detail
                );
                let _ = on_batch.send(DirBatch {
                    gen,
                    scanned: 0,
                    done: true,
                    truncated: false,
                    items: Vec::new(),
                    error: Some(err),
                });
                return;
            }
        };
        reported = entries.len();
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
                        // Keep the readout at the enumeration total while rows stream in.
                        scanned: (reported as u32).max(scanned),
                        done: false,
                        truncated: false,
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
        log::info!(
            target: "flowdir::fs",
            "list_dir done rows={} truncated={truncated} in {}ms",
            (reported as u32).max(scanned),
            started.elapsed().as_millis(),
        );
        log::debug!(target: "flowdir::fs", "list_dir done path={path}");
        let _ = on_batch.send(DirBatch {
            gen,
            scanned: (reported as u32).max(scanned),
            done: true,
            truncated,
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
    shutdown(&app, true);
}

/// Asks the frontend how many operations are running so quitting can confirm first.
#[tauri::command]
fn active_ops(state: tauri::State<'_, ops::OpState>) -> Vec<String> {
    state.active()
}

/// Requests a graceful quit from the tray menu or the frontend.
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    shutdown(&app, false);
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
    let started = std::time::Instant::now();
    let status = tauri::async_runtime::spawn_blocking(move || git::status(&path))
        .await
        .map_err(|e| ApiError::new("GIT_TASK", e.to_string()))?;
    log::debug!(
        target: "flowdir::git",
        "git_status root={} in {}ms",
        status.root,
        started.elapsed().as_millis()
    );
    Ok(status)
}

/// Subdirectory names of one folder, plus whether the caps cut them short.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TreeListing {
    names: Vec<String>,
    /// True when either the scan cap or the row cap cut the folder's children.
    truncated: bool,
}

/// Children for the sidebar tree. Capped so a pathological folder cannot flood the panel;
/// the tree only asks for expanded nodes.
#[tauri::command(rename_all = "camelCase")]
async fn tree_dirs(path: String, hidden: bool) -> Result<TreeListing, ApiError> {
    tauri::async_runtime::spawn_blocking(move || {
        let cancel = AtomicBool::new(false);
        let outcome = fs::enumerate_dirs(&path, hidden, TREE_SCAN_LIMIT, &cancel)?;
        let mut entries = outcome.entries;
        fs::sort_entries(&mut entries, "name", "asc");
        let truncated = outcome.truncated || entries.len() > TREE_LIMIT;
        entries.truncate(TREE_LIMIT);
        Ok(TreeListing {
            names: entries.into_iter().map(|entry| entry.name).collect(),
            truncated,
        })
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
async fn quick_access() -> Vec<fs::Place> {
    tauri::async_runtime::spawn_blocking(fs::quick_access)
        .await
        .unwrap_or_default()
}

/// Which of `paths` already exist, for a rename/move preview that must not overwrite.
#[tauri::command]
async fn paths_exist(paths: Vec<String>) -> Vec<bool> {
    let count = paths.len();
    tauri::async_runtime::spawn_blocking(move || naming::existing(&paths))
        .await
        .unwrap_or_else(|_| vec![false; count])
}

/// Free "name (n)" variants of every path, in the same order as the input.
#[tauri::command]
async fn unique_paths(paths: Vec<String>) -> Vec<String> {
    let fallback = paths.clone();
    tauri::async_runtime::spawn_blocking(move || naming::unique(&paths))
        .await
        .unwrap_or(fallback)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SortItemIn {
    name: String,
    size: u64,
    mtime: u64,
    is_dir: bool,
}

/// Display order for rows already in the frontend: changing the sort column must not
/// re-enumerate the folder. Uses the same comparator as a fresh scan, so the result is
/// identical to what `list_dir` would return.
#[tauri::command]
async fn sort_order(items: Vec<SortItemIn>, key: String, dir: String) -> Vec<u32> {
    let fallback: Vec<u32> = (0..items.len() as u32).collect();
    tauri::async_runtime::spawn_blocking(move || {
        let rows: Vec<fs::SortItem> = items
            .into_iter()
            .map(|item| fs::SortItem {
                name: item.name,
                size: item.size,
                mtime: item.mtime,
                is_dir: item.is_dir,
            })
            .collect();
        fs::sort_order(&rows, &key, &dir)
    })
    .await
    .unwrap_or(fallback)
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
) -> Result<Vec<String>, ApiError> {
    let settings = settings.normalize();
    settings::save(&app, &settings)?;
    crate::logging::set_level(&settings.log_level);
    // Integration switches apply immediately; the returned codes let the frontend toast a
    // hotkey that another program already owns.
    let warnings = shell_integration::apply(&app, &settings);
    cache.set(settings);
    Ok(warnings)
}

#[tauri::command]
fn take_launch_request(
    state: tauri::State<'_, shell_integration::Integration>,
) -> Option<shell_integration::LaunchRequest> {
    state.take_launch()
}

#[tauri::command]
fn tray_recent(app: tauri::AppHandle, dirs: Vec<String>) -> Result<(), ApiError> {
    shell_integration::set_recent(&app, dirs)
}

#[tauri::command]
async fn watch_set(
    app: tauri::AppHandle,
    pane_id: String,
    path: String,
) -> Result<(), ApiError> {
    tauri::async_runtime::spawn_blocking(move || {
        let watchers = app.state::<watch::Watchers>();
        watch::set(&app, &watchers, pane_id, path)
    })
    .await
    .map_err(|e| ApiError::new("WATCH_TASK", e.to_string()))?
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
    new_names: Option<Vec<String>>,
    permanent: bool,
    overwrite: bool,
    on_progress: Channel<ops::OpProgress>,
) -> Result<(), ApiError> {
    let hwnd = app
        .get_webview_window("main")
        .and_then(|window| window.hwnd().ok())
        .map(|hwnd| hwnd.0 as isize);
    log::info!(
        target: "flowdir::ops",
        "op_start kind={kind} items={} permanent={permanent}",
        sources.len(),
    );
    log::debug!(target: "flowdir::ops", "op target={target_dir:?}");
    ops::start(
        &state,
        ops::Request {
            op_id,
            kind,
            sources,
            target_dir,
            new_name,
            new_names,
            permanent,
            overwrite,
        },
        hwnd,
        on_progress,
    )
}

#[tauri::command]
fn op_cancel(state: tauri::State<'_, ops::OpState>, op_id: String) {
    state.cancel(&op_id);
}

/// Extracts one archive into `dest`; progress arrives on the channel and `op_cancel`
/// stops it, because extraction registers itself in the shared op state.
#[tauri::command(rename_all = "camelCase")]
fn archive_extract(
    state: tauri::State<'_, ops::OpState>,
    job_id: String,
    archive: String,
    dest: String,
    on_progress: Channel<ops::OpProgress>,
) -> Result<(), ApiError> {
    archive::start(&state, job_id, archive, dest, on_progress)
}

/// Starts a checksum job; progress arrives on the channel and `tool_cancel` stops it.
#[tauri::command(rename_all = "camelCase")]
fn hash_start(
    state: tauri::State<'_, hash::ToolState>,
    job_id: String,
    paths: Vec<String>,
    algorithm: String,
    on_progress: Channel<hash::HashProgress>,
) -> Result<(), ApiError> {
    log::info!(target: "flowdir::hash", "hash_start items={} algo={algorithm}", paths.len());
    hash::start_hash(&state, job_id, paths, algorithm, on_progress)
}

/// Lists one level of the two compare roots; the frontend expands a folder by asking for
/// that folder's level, so nothing below it is read until it is opened.
#[tauri::command(rename_all = "camelCase")]
fn compare_level(
    left: String,
    right: String,
    prefix: String,
    options: compare::Options,
) -> Result<compare::Level, ApiError> {
    compare::level(&left, &right, &prefix, &options)
}

/// Recursive difference counts for the shared child folders of one level; separately
/// cancellable so a slow network share cannot hold the first screen hostage.
#[tauri::command(rename_all = "camelCase")]
fn compare_counts_start(
    state: tauri::State<'_, hash::ToolState>,
    job_id: String,
    left: String,
    right: String,
    prefix: String,
    options: compare::Options,
    on_event: Channel<compare::CountEvent>,
) -> Result<(), ApiError> {
    compare::start_counts(&state, job_id, left, right, prefix, options, on_event)
}

/// SHA-256 verification for pairs the user asked to confirm; shares the byte rate limit
/// with the duplicate scanner.
#[tauri::command(rename_all = "camelCase")]
fn compare_hash_start(
    state: tauri::State<'_, hash::ToolState>,
    job_id: String,
    pairs: Vec<compare::HashPair>,
    max_bytes_per_sec: u64,
    on_event: Channel<compare::HashEvent>,
) -> Result<(), ApiError> {
    compare::start_hash(&state, job_id, pairs, max_bytes_per_sec, on_event)
}

/// Starts the three-phase duplicate scan. Concurrency and rate come from settings; the
/// backend re-validates them so a hand-edited file cannot create hundreds of workers.
#[tauri::command(rename_all = "camelCase")]
fn dup_start(
    state: tauri::State<'_, hash::ToolState>,
    job_id: String,
    roots: Vec<String>,
    include_hidden: bool,
    min_size: u64,
    concurrency: u32,
    max_bytes_per_sec: u64,
    exclude: Vec<String>,
    on_progress: Channel<dup::DupProgress>,
) -> Result<(), ApiError> {
    log::info!(target: "flowdir::hash", "dup_start roots={} workers={concurrency}", roots.len());
    dup::start(
        &state,
        job_id,
        roots,
        include_hidden,
        min_size,
        concurrency.min(16),
        max_bytes_per_sec,
        exclude,
        on_progress,
    )
}

#[tauri::command]
fn tool_cancel(state: tauri::State<'_, hash::ToolState>, job_id: String) {
    state.cancel(&job_id);
}

#[tauri::command]
fn tool_pause(state: tauri::State<'_, hash::ToolState>, job_id: String, paused: bool) {
    state.pause(&job_id, paused);
}

/// Retries a denied operation under an elevated helper. The frontend offers this only after
/// the ordinary run reported `ACCESS_DENIED`; the GUI itself stays non-elevated.
#[tauri::command]
async fn op_retry_elevated(
    app: tauri::AppHandle,
    state: tauri::State<'_, ops::OpState>,
    request: ops::Request,
) -> Result<ops::ElevatedReport, ApiError> {
    log::info!(
        target: "flowdir::ops",
        "op_retry_elevated kind={} items={}",
        request.kind,
        request.sources.len(),
    );
    // Only an operation that really failed with ACCESS_DENIED, with identical arguments,
    // may be elevated. The denied record is consumed so a retry cannot be replayed.
    let state = state.inner().clone();
    if !state.take_denied(&request) {
        return Err(ApiError::new(
            "ELEVATED_UNBOUND",
            "no denied operation matches this retry",
        ));
    }
    let token = state.begin_external(&request.op_id);
    let cancel = token.clone();
    let op_id = request.op_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        elevated::request_elevated(&app, request, cancel)
    })
    .await
    .map_err(|err| ApiError::new("THREAD", err.to_string()))?;
    state.finish_external(&op_id);
    result
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
    hash_label: Option<String>,
) -> Result<shell_menu::Outcome, ApiError> {
    tauri::async_runtime::spawn_blocking(move || shell_menu::show(paths, x, y, extended, hash_label))
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
    dup_label: String,
    compare_label: String,
) -> Result<shell_menu::Outcome, ApiError> {
    tauri::async_runtime::spawn_blocking(move || {
        shell_menu::show_background(path, x, y, extended, toggle_label, dup_label, compare_label)
    })
    .await
    .map_err(|e| ApiError::new("THREAD", e.to_string()))?
}

/// Writes an exported report into a user-picked folder and returns its full path.
#[tauri::command]
fn export_text(dir: String, name: String, content: String) -> Result<String, ApiError> {
    let clean: String = name
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    if clean.trim().is_empty() {
        return Err(ApiError::new("INVALID_NAME", "empty export name"));
    }
    let path = std::path::Path::new(&dir).join(&clean);
    std::fs::write(&path, content).map_err(|e| ApiError::from_io(&path.display().to_string(), &e))?;
    Ok(path.display().to_string())
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
    hydrate: Option<bool>,
) -> Result<preview::PreviewInfo, ApiError> {
    let bg = u32::from_str_radix(bg.trim_start_matches('#'), 16).unwrap_or(0xffffff);
    // Archive central directories and version resources can sit on a network share:
    // never inspect a file on the IPC thread.
    let settings = cache.get(&app);
    let external = settings.external_preview.clone();
    let limits = preview::Limits::from_settings(&settings);
    tauri::async_runtime::spawn_blocking(move || {
        preview::info(path, width, bg, &external, limits, hydrate.unwrap_or(false))
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
    hidden: bool,
    filter: search::SearchFilter,
    content: search::ContentOptions,
    on_batch: Channel<search::SearchBatch>,
) -> Result<(), ApiError> {
    let cancel = state.begin();
    let started = std::time::Instant::now();
    let log_root = root.clone();
    log::info!(target: "flowdir::search", "search_start gen={gen} limit={limit}");
    log::debug!(target: "flowdir::search", "search root={root}");
    let _ = tauri::async_runtime::spawn_blocking(move || {
        search::run(
            root,
            query,
            gen,
            limit,
            prefer_everything,
            hidden,
            filter,
            content,
            cancel,
            on_batch,
        );
        log::info!(
            target: "flowdir::search",
            "search done gen={gen} in {}ms",
            started.elapsed().as_millis()
        );
        log::debug!(target: "flowdir::search", "search root={log_root}");
    });
    Ok(())
}

#[tauri::command]
fn set_app_theme(app: tauri::AppHandle, theme: String) {
    let window = app.get_webview_window("main");
    let hwnd = window
        .as_ref()
        .and_then(|window| window.hwnd().ok())
        .map(|hwnd| windows::Win32::Foundation::HWND(hwnd.0));
    theme::apply(hwnd, &theme);
    // Keep the fill the parked webview reveals in step with the theme it wakes into.
    if let Some(window) = window {
        let _ = window.set_background_color(Some(theme::surface(&theme)));
    }
}

#[tauri::command]
async fn everything_status() -> search::EverythingStatus {
    tauri::async_runtime::spawn_blocking(search::everything_status)
        .await
        .unwrap_or_default()
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

/// Puts recycled files back at their original paths (Ctrl+Z after a delete).
#[tauri::command]
async fn restore_recycle(paths: Vec<String>) -> Result<restore::RestoreReport, ApiError> {
    tauri::async_runtime::spawn_blocking(move || restore::restore(&paths))
        .await
        .map_err(|e| ApiError::new("RESTORE_TASK", e.to_string()))?
}

/// Taskbar progress for long operations; `state` is normal/error/paused/none.
#[tauri::command]
fn taskbar_progress(
    app: tauri::AppHandle,
    state: String,
    completed: u64,
    total: u64,
) -> Result<(), ApiError> {
    let hwnd = app
        .get_webview_window("main")
        .and_then(|window| window.hwnd().ok())
        .ok_or_else(|| ApiError::new("NO_WINDOW", "main window is gone"))?;
    taskbar::progress(hwnd.0 as isize, &state, completed, total)
}

/// Background completion reminder: a system toast plus a taskbar-button flash.
#[tauri::command]
fn notify_op(app: tauri::AppHandle, title: String, body: String) -> Result<(), ApiError> {
    use tauri_plugin_notification::NotificationExt;
    if let Some(hwnd) = app
        .get_webview_window("main")
        .and_then(|window| window.hwnd().ok())
    {
        taskbar::flash(hwnd.0 as isize);
    }
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| ApiError::new("NOTIFY", e.to_string()))
}

/// Frontend exceptions, unhandled rejections and render errors land in the same file.
#[tauri::command]
fn log_frontend(level: String, message: String, detail: String) {
    logging::frontend(&level, &message, &detail);
}

#[tauri::command]
fn log_dir_path() -> Option<String> {
    logging::dir().map(|dir| dir.to_string_lossy().into_owned())
}

/// Writes logs, version facts and a redacted settings snapshot into `dir`.
#[tauri::command]
async fn diagnostics_export(
    app: tauri::AppHandle,
    cache: tauri::State<'_, settings::SettingsCache>,
    dir: String,
) -> Result<String, ApiError> {
    let settings = cache.get(&app);
    tauri::async_runtime::spawn_blocking(move || logging::export(&app, &settings, &dir))
        .await
        .map_err(|e| ApiError::new("EXPORT_TASK", e.to_string()))?
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
    let started = std::time::Instant::now();
    let status = tauri::async_runtime::spawn_blocking(move || update::check(&app, force))
        .await
        .map_err(|e| ApiError::new("UPDATE_TASK", e.to_string()))?;
    match &status.error {
        Some(err) => log::warn!(
            target: "flowdir::update",
            "update_check failed: {} {}",
            err.code,
            err.detail
        ),
        None => log::info!(
            target: "flowdir::update",
            "update_check latest={:?} available={} checked={} in {}ms",
            status.latest,
            status.available,
            status.checked,
            started.elapsed().as_millis(),
        ),
    }
    Ok(status)
}

#[tauri::command]
async fn update_install(app: tauri::AppHandle) -> Result<(), ApiError> {
    log::info!(target: "flowdir::update", "update_install start");
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
fn config_warnings() -> Vec<&'static str> {
    let mut warnings = Vec::new();
    if settings::CONFIG_REJECTED.swap(false, std::sync::atomic::Ordering::Relaxed) {
        warnings.push("SETTINGS_REJECTED");
    }
    if layout::LAYOUT_REJECTED.swap(false, std::sync::atomic::Ordering::Relaxed) {
        warnings.push("LAYOUT_REJECTED");
    }
    warnings
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
    // ShellExecuteW reports failure through its return value, not GetLastError.
    let code = result.0 as isize;
    if code > 32 {
        return Ok(());
    }
    Err(match code {
        // The shell has no default handler for this extension; the caller offers the
        // system "Open with" dialog instead of leaving the double-click silent.
        31 => ApiError::new("NO_ASSOCIATION", path),
        2 => ApiError::new("NOT_FOUND", path),
        5 => ApiError::new("ACCESS_DENIED", path),
        _ => ApiError::new("SHELL_FAILED", format!("ShellExecuteW 返回 {code}")),
    })
}

/// Shows the system "Open with" dialog for a file that has no default program.
///
/// `SHOpenWithDialog` is modal and needs a single-threaded apartment, so it runs on a
/// blocking thread with its own COM initialisation instead of a shared runtime worker.
#[tauri::command]
async fn open_with(path: String) -> Result<(), ApiError> {
    use windows::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };
    use windows::Win32::UI::Shell::{
        SHOpenWithDialog, OAIF_ALLOW_REGISTRATION, OAIF_EXEC, OPENASINFO,
    };
    let join = tauri::async_runtime::spawn_blocking(move || {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
            if hr.is_err() && hr.0 as u32 != 0x8001_0106 {
                // S_FALSE means the apartment was already initialised on this thread; any
                // other failure leaves no working COM, so the dialog cannot be shown.
                return Err(ApiError::new("SHELL_FAILED", hr.message()));
            }
            let info = OPENASINFO {
                pcszFile: PCWSTR(wide.as_ptr()),
                pcszClass: PCWSTR::null(),
                // Registration stays allowed so "always use this app" works, and
                // OAIF_EXEC launches the chosen program immediately.
                oaifInFlags: OAIF_ALLOW_REGISTRATION | OAIF_EXEC,
            };
            let result = SHOpenWithDialog(None, &info);
            CoUninitialize();
            result.map_err(|err| {
                let raw = err.code().0 as u32;
                if raw == 0x8007_04C7 {
                    // The user dismissed the dialog; not a failure worth reporting.
                    ApiError::new("CANCELED", "open-with dialog dismissed")
                } else {
                    ApiError::new("SHELL_FAILED", err.message())
                }
            })
        }
    })
    .await
    .map_err(|err| ApiError::new("IO", err.to_string()))?;
    join
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

pub(crate) fn show_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// The single exit path. The window close handler already flushes layout; a tray quit,
/// restart or update install bypassed it and could drop the last settings/layout write or
/// kill a running copy mid-flight.
fn shutdown<R: tauri::Runtime>(app: &tauri::AppHandle<R>, restart: bool) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.emit("flowdir:flush-layout", ());
    }
    let ops = app.state::<ops::OpState>();
    if !ops.active().is_empty() {
        ops.cancel_all();
        ops.wait_idle(std::time::Duration::from_secs(5));
    }
    if restart {
        app.restart();
    } else {
        app.exit(0);
    }
}

/// Asks the frontend to confirm before quitting while a copy/move is still running.
/// Returns true when the quit was deferred to that confirmation.
fn confirm_if_busy<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    let active = app.state::<ops::OpState>().active();
    if active.is_empty() {
        return false;
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.emit("flowdir:confirm-quit", active.len());
        let _ = window.set_focus();
    }
    true
}

/// Runs the same flush/cancel sequence as `shutdown`, then exits when it finished.
fn request_quit<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if !confirm_if_busy(app) {
        shutdown(app, false);
    }
}

fn init_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let menu = shell_integration::tray_menu(app, &[])?;
    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().unwrap())
        .tooltip(store::display_name(app))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "quit" => request_quit(app),
            id => shell_integration::on_menu_event(app, id),
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
    // The elevated helper must not start Tauri: no window, no tray, no single-instance
    // mutex, and no second WebView2 process for a filesystem operation.
    if let Some((pipe, nonce)) = elevated::helper_args() {
        std::process::exit(elevated::run_helper(&pipe, &nonce));
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // `argv` includes the executable, as `std::env::args` does.
            let request = shell_integration::parse_launch(argv.into_iter().skip(1));
            if request.autostart {
                // The Run entry firing while the app is already up: stay in the tray.
                return;
            }
            show_main(app);
            if !request.is_empty() {
                let _ = app.emit("flowdir:open-request", request);
            }
        }))
        .manage(AppState::default())
        .manage(shell_integration::Integration::default())
        .manage(ops::OpState::default())
        .manage(hash::ToolState::default())
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
            // The frontend asks for this once its workspace is restored, so a path from
            // Explorer or the command line lands on a ready list.
            let launch = shell_integration::parse_launch(std::env::args().skip(1));
            app.state::<shell_integration::Integration>()
                .set_launch(launch.clone());
            let saved = settings::load(app.handle());
            // The logger starts before the window so every later failure has somewhere to go.
            if let Ok(cache) = store::cache_dir(app.handle()) {
                let _ = logging::init(cache.join("logs"), &saved.log_level);
            }
            logging::install_panic_hook();
            logging::log_startup(app.handle());
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
            // The webview paints nothing while it is parked or still loading; this is what
            // the window shows instead, so it has to be the app surface, not white.
            .background_color(theme::surface(&saved.theme))
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
            // Autostart can stay in the tray; the hotkey and tray menu still work.
            let hidden = launch.autostart && saved.autostart_minimized;
            if !hidden {
                let _ = window.show();
                let _ = window.set_focus();
            }
            if let Err(err) = init_tray(app.handle()) {
                eprintln!("tray init failed: {err}");
            }
            for code in shell_integration::apply(app.handle(), &saved) {
                log::warn!(target: "flowdir::shell", "integration: {code}");
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
                        // A running copy/move needs the user's word before the window goes
                        // away; the frontend answers with `quit_app`. Cancelling restores
                        // the normal close path.
                        if confirm_if_busy(win.app_handle()) {
                            win.state::<AppState>().closing.store(false, Ordering::SeqCst);
                        } else {
                            let _ = win.close();
                        }
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
            paths_exist,
            unique_paths,
            sort_order,
            git_status,
            quick_access,
            open_path,
            open_with,
            explorer_open,
            reveal_path,
            layout_save,
            layout_preserve,
            layout_load,
            settings_load,
            settings_save,
            take_launch_request,
            tray_recent,
            config_path,
            restart_app,
            quit_app,
            active_ops,
            pick_folder,
            set_app_theme,
            watch_set,
            watch_clear,
            clipboard_copy,
            clipboard_paste,
            drag_out,
            op_start,
            op_cancel,
            op_retry_elevated,
            archive_extract,
            hash_start,
            dup_start,
            compare_level,
            compare_counts_start,
            compare_hash_start,
            tool_cancel,
            tool_pause,
            shell_menu_show,
            shell_menu_background_show,
            export_text,
            titlebar_menu,
            preview_info,
            preview_external,
            search_start,
            search_cancel,
            everything_status,
            shell_properties,
            drive_eject,
            restore_recycle,
            taskbar_progress,
            notify_op,
            log_frontend,
            log_dir_path,
            diagnostics_export,
            app_info,
            config_warnings,
            update_check,
            update_install
        ])
        .run(tauri::generate_context!())
        .expect("FlowDir failed to start");
}
