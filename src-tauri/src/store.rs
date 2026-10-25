use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager, Runtime};

use crate::error::ApiError;

/// Folder for settings and session state, deliberately independent of the bundle
/// identifier so the on-disk location stays predictable for users.
pub const APP_DIR: &str = "FlowDir";

/// Pre-migration folder, derived from the bundle identifier.
const LEGACY_DIR: &str = "dev.flowdir.app";

/// `%APPDATA%\FlowDir`
pub fn config_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, ApiError> {
    app.path()
        .data_dir()
        .map(|dir| dir.join(APP_DIR))
        .map_err(|e| ApiError::new("CONFIG_DIR", e.to_string()))
}

/// `%LOCALAPPDATA%\FlowDir`
pub fn cache_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, ApiError> {
    app.path()
        .local_data_dir()
        .map(|dir| dir.join(APP_DIR))
        .map_err(|e| ApiError::new("CONFIG_DIR", e.to_string()))
}

/// Moves settings, layout and the WebView2 profile out of the identifier-derived folder
/// into `%APPDATA%\FlowDir` and `%LOCALAPPDATA%\FlowDir`. Runs before the window exists so
/// the browser profile is not in use; every step is a no-op once migrated.
pub fn migrate_legacy<R: Runtime>(app: &AppHandle<R>) {
    if let (Ok(new_config), Ok(base)) = (config_dir(app), app.path().data_dir()) {
        let old_config = base.join(LEGACY_DIR);
        if old_config != new_config && old_config.is_dir() {
            let _ = std::fs::create_dir_all(&new_config);
            for name in [
                "layout.json",
                "layout.json.bak",
                "layout.rejected.json",
                "settings.json",
                "settings.json.bak",
            ] {
                let src = old_config.join(name);
                let dst = new_config.join(name);
                if src.is_file() && !dst.exists() {
                    let _ = std::fs::rename(&src, &dst);
                }
            }
            // Only succeeds when nothing but the moved files lived there.
            let _ = std::fs::remove_dir(&old_config);
        }
    }

    if let (Ok(new_cache), Ok(base)) = (cache_dir(app), app.path().local_data_dir()) {
        let old_cache = base.join(LEGACY_DIR);
        let profile = "EBWebView";
        let src = old_cache.join(profile);
        let dst = new_cache.join(profile);
        if old_cache != new_cache && src.is_dir() && !dst.exists() {
            let _ = std::fs::create_dir_all(&new_cache);
            let _ = std::fs::rename(&src, &dst);
            let _ = std::fs::remove_dir(&old_cache);
        }
    }
}

/// Writes `bytes` to `path` atomically, keeping the previous file as `.bak`.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_file_name(format!(
        "{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file")
    ));
    std::fs::write(&tmp, bytes)?;
    if path.exists() {
        let backup = path.with_file_name(format!(
            "{}.bak",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("file")
        ));
        let _ = std::fs::copy(path, backup);
    }
    std::fs::rename(&tmp, path)
}
