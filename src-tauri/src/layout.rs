use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Runtime};

use crate::error::ApiError;
use crate::store;

pub const VERSION: u32 = 1;

/// Set when the primary layout file was unusable, so the frontend can tell the user.
pub static LAYOUT_REJECTED: AtomicBool = AtomicBool::new(false);
/// Set when the primary file existed but could not be read (as opposed to missing or
/// corrupt). The backup is used, but the user should know the last snapshot was skipped.
pub static LAYOUT_UNREADABLE: AtomicBool = AtomicBool::new(false);
/// Sticky form of [`LAYOUT_UNREADABLE`] for the save guard. The reporting flag is cleared
/// by `config_warnings`, but an autosave must stay blocked for the rest of the run:
/// writing the fallback layout back would replace a file that was merely inaccessible.
static LAYOUT_LOAD_FAILED: AtomicBool = AtomicBool::new(false);

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
        Read::Unreadable => {
            // A sharing violation is not "no layout": going straight to `None` would let
            // the next autosave replace a perfectly good file with the default layout.
            LAYOUT_UNREADABLE.store(true, Ordering::Relaxed);
            LAYOUT_LOAD_FAILED.store(true, Ordering::Relaxed);
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
    /// The file exists but could not be opened or read (locking, sharing violation).
    Unreadable,
}

fn read(path: &std::path::Path) -> Read {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Read::Missing,
        Err(_) => return Read::Unreadable,
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

/// Whether `load` fell back to the backup because the primary file was unreadable.
///
/// Saves must stay disabled in that state: the workspace in memory came from the backup
/// (or defaults), so writing it back would replace the file that was only momentarily
/// locked with an older snapshot.
pub fn load_failed() -> bool {
    LAYOUT_LOAD_FAILED.load(Ordering::Relaxed)
}

pub fn save<R: Runtime>(app: &AppHandle<R>, workspace: Value) -> Result<(), ApiError> {
    if load_failed() {
        return Err(ApiError::new(
            "CONFIG_UNREADABLE",
            "layout.json could not be read; restart before saving",
        ));
    }
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
    // A degenerate work area (0×0 from a detached display or RDP session) leaves no room
    // for the 200-px floor, which made `max_x < area_x` and panicked `clamp` during setup —
    // a crash loop until layout.json was deleted. Anchor such a window at the area corner.
    let max_x = (area_x as i64 + area_w as i64 - width as i64).max(area_x as i64);
    let max_y = (area_y as i64 + area_h as i64 - height as i64).max(area_y as i64);
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
    let monitors = window.available_monitors().unwrap_or_default();
    let chosen = state
        .monitor
        .as_ref()
        .and_then(|name| monitors.iter().find(|monitor| monitor.name() == Some(name)))
        .or_else(|| monitors.iter().find(|monitor| intersects(monitor, &state)))
        .or_else(|| monitors.first());
    if state.maximized {
        // Maximizing before the window reaches the saved monitor parks it on whichever
        // monitor currently owns it (usually the primary).
        if let Some(monitor) = chosen {
            let work = monitor.work_area();
            let _ = window.set_position(PhysicalPosition::new(work.position.x, work.position.y));
        }
        let _ = window.maximize();
        return;
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn state(x: i32, y: i32, width: u32, height: u32) -> WindowState {
        WindowState {
            x,
            y,
            width,
            height,
            maximized: false,
            monitor: None,
        }
    }

    #[test]
    fn clamp_survives_a_degenerate_work_area() {
        // `rcWork` is 0×0 on a detached display; the 200×120 floor used to invert the
        // clamp range and panic inside `setup`, which made every launch crash.
        let clamped = clamp_to((0, 0, 0, 0), &state(5000, 4000, 900, 600));
        assert_eq!((clamped.x, clamped.y), (0, 0));
        assert_eq!((clamped.width, clamped.height), (200, 120));
    }

    #[test]
    fn clamp_keeps_the_saved_rectangle_inside_a_real_area() {
        let clamped = clamp_to((0, 0, 1920, 1080), &state(5000, 4000, 900, 600));
        assert_eq!((clamped.x, clamped.width), (1020, 900));
        assert_eq!((clamped.y, clamped.height), (480, 600));
        let smaller = clamp_to((0, 0, 1920, 1080), &state(10, 20, 120, 80));
        assert_eq!((smaller.x, smaller.width), (10, 200));
        assert_eq!((smaller.y, smaller.height), (20, 120));
    }
}
