use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Runtime};

use crate::error::ApiError;
use crate::store;

pub const VERSION: u32 = 1;

/// Set when the primary layout file was unusable, so the frontend can tell the user.
pub static LAYOUT_REJECTED: AtomicBool = AtomicBool::new(false);

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
    /// `MONITORINFOEX.szDevice`, so a window returns to the same physical screen.
    #[serde(default)]
    pub monitor: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LayoutFile {
    pub version: u32,
    #[serde(default)]
    pub window: Option<WindowState>,
    pub workspace: Value,
}

fn path_for<R: Runtime>(app: &AppHandle<R>) -> Result<std::path::PathBuf, ApiError> {
    Ok(store::config_dir(app)?.join("layout.json"))
}

pub fn load<R: Runtime>(app: &AppHandle<R>) -> Option<LayoutFile> {
    let path = path_for(app).ok()?;
    match read(&path) {
        Read::Ok(file) => return Some(file),
        Read::Invalid => {
            LAYOUT_REJECTED.store(true, Ordering::Relaxed);
        }
        Read::Missing => return None,
    }
    let backup = path.with_file_name("layout.json.bak");
    match read(&backup) {
        Read::Ok(file) => Some(file),
        _ => None,
    }
}

enum Read {
    Ok(LayoutFile),
    Invalid,
    Missing,
}

fn read(path: &std::path::Path) -> Read {
    let Ok(bytes) = std::fs::read(path) else {
        return Read::Missing;
    };
    serde_json::from_slice::<LayoutFile>(&bytes)
        .ok()
        .filter(|file| file.version == VERSION)
        .map(Read::Ok)
        .unwrap_or(Read::Invalid)
}

/// Copies the current layout file aside before a fallback workspace overwrites it.
/// A rejected snapshot (unknown shape, clipped ids) is still the only record of what the
/// user had open, so it is kept instead of being lost to the next autosave.
pub fn preserve<R: Runtime>(app: &AppHandle<R>) -> Result<std::path::PathBuf, ApiError> {
    let path = path_for(app)?;
    let backup = path.with_file_name("layout.rejected.json");
    std::fs::copy(&path, &backup).map_err(|e| ApiError::new("IO", e.to_string()))?;
    Ok(backup)
}

pub fn save<R: Runtime>(app: &AppHandle<R>, workspace: Value) -> Result<(), ApiError> {
    let path = path_for(app)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| ApiError::new("CONFIG_DIR", e.to_string()))?;
    }
    let file = LayoutFile {
        version: VERSION,
        window: current_window_state(app),
        workspace,
    };
    let bytes =
        serde_json::to_vec_pretty(&file).map_err(|e| ApiError::new("SERIALIZE", e.to_string()))?;
    store::write_atomic(&path, &bytes).map_err(|e| ApiError::new("IO", e.to_string()))
}

fn current_window_state<R: Runtime>(app: &AppHandle<R>) -> Option<WindowState> {
    let window = app.get_webview_window("main")?;
    let position = window.outer_position().ok()?;
    let size = window.outer_size().ok()?;
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .and_then(|monitor| monitor.name().cloned());
    Some(WindowState {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        maximized: window.is_maximized().unwrap_or(false),
        monitor,
    })
}

/// Clamps a saved rectangle into the work area of the monitor it was last seen on.
/// Arithmetic is done in `i64` because hand-edited layout.json may contain values that
/// would overflow `i32` and panic a debug build.
fn clamp_to(work: (i32, i32, u32, u32), state: &WindowState) -> WindowState {
    let (area_x, area_y, area_w, area_h) = work;
    let width = (state.width as i64).clamp(200, area_w.max(200) as i64) as u32;
    let height = (state.height as i64).clamp(120, area_h.max(120) as i64) as u32;
    let max_x = area_x as i64 + area_w as i64 - width as i64;
    let max_y = area_y as i64 + area_h as i64 - height as i64;
    let x = (state.x as i64).clamp(area_x as i64, max_x) as i32;
    let y = (state.y as i64).clamp(area_y as i64, max_y) as i32;
    WindowState {
        x,
        y,
        width,
        height,
        maximized: state.maximized,
        monitor: state.monitor.clone(),
    }
}

fn intersects(monitor: &tauri::Monitor, state: &WindowState) -> bool {
    let position = monitor.position();
    let size = monitor.size();
    let left = position.x as i64;
    let top = position.y as i64;
    let right = left + size.width as i64;
    let bottom = top + size.height as i64;
    let x = state.x as i64;
    let y = state.y as i64;
    x < right && x + state.width as i64 > left && y < bottom && y + state.height as i64 > top
}

/// Applies the persisted window geometry, returning it to its last monitor when that
/// monitor still exists and otherwise to the nearest visible work area.
pub fn restore_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(file) = load(app) else {
        return;
    };
    let Some(state) = file.window else {
        return;
    };
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if state.maximized {
        let _ = window.maximize();
        return;
    }

    let monitors = window.available_monitors().unwrap_or_default();
    let chosen = state
        .monitor
        .as_ref()
        .and_then(|name| monitors.iter().find(|monitor| monitor.name() == Some(name)))
        .or_else(|| monitors.iter().find(|monitor| intersects(monitor, &state)))
        .or_else(|| monitors.first());
    let Some(monitor) = chosen else {
        return;
    };
    let work = monitor.work_area();
    let clamped = clamp_to(
        (work.position.x, work.position.y, work.size.width, work.size.height),
        &state,
    );
    let _ = window.set_position(PhysicalPosition::new(clamped.x, clamped.y));
    let _ = window.set_size(PhysicalSize::new(clamped.width, clamped.height));
}
