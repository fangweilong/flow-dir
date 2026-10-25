use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Runtime};

use crate::error::ApiError;
use crate::store;

pub const VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
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
    let bytes = std::fs::read(path).ok()?;
    let file: LayoutFile = serde_json::from_slice(&bytes).ok()?;
    if file.version != VERSION {
        return None;
    }
    Some(file)
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
    Some(WindowState {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        maximized: window.is_maximized().unwrap_or(false),
    })
}

/// Applies the persisted window geometry, ignoring positions outside every monitor.
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

    let visible = window
        .available_monitors()
        .map(|monitors| {
            monitors.iter().any(|monitor| {
                let position = monitor.position();
                let size = monitor.size();
                let right = position.x + size.width as i32;
                let bottom = position.y + size.height as i32;
                state.x < right
                    && state.x + state.width as i32 > position.x
                    && state.y < bottom
                    && state.y + state.height as i32 > position.y
            })
        })
        .unwrap_or(true);

    if visible {
        let _ = window.set_position(PhysicalPosition::new(state.x, state.y));
        let _ = window.set_size(PhysicalSize::new(state.width, state.height));
    }
}
