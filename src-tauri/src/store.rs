use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Manager, Runtime};

use crate::error::ApiError;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);
/// Serializes writers in this process; two auto-save paths must not rotate `.bak` while
/// the other is renaming its temporary file.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Folder for settings and session state, deliberately independent of the bundle
/// identifier so the on-disk location stays predictable for users.
pub const APP_DIR: &str = "FlowDir";

/// Folder used by development builds; the release keeps [`APP_DIR`].
const DEV_APP_DIR: &str = "FlowDir-dev";

/// Identifier suffix that marks a development build. `tauri.dev.conf.json` sets
/// `dev.flowdir.app.dev`, and the plugin derives its single-instance mutex from the same
/// identifier, so release and dev runs see independent locks. Keeping the switch here means
/// the dev config is the only place to change it.
const DEV_IDENTIFIER_SUFFIX: &str = ".dev";

/// Pre-migration folder, derived from the bundle identifier.
const LEGACY_DIR: &str = "dev.flowdir.app";

/// True when this binary was compiled with `tauri.dev.conf.json`.
pub fn is_dev<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.config().identifier.ends_with(DEV_IDENTIFIER_SUFFIX)
}

/// `FlowDir` or `FlowDir-dev`.
fn dir_name<R: Runtime>(app: &AppHandle<R>) -> &'static str {
    if is_dev(app) {
        DEV_APP_DIR
    } else {
        APP_DIR
    }
}

/// Window title and tray tooltip, so both builds are distinguishable in the taskbar.
pub fn display_name<R: Runtime>(app: &AppHandle<R>) -> &'static str {
    if is_dev(app) {
        "FlowDir Dev"
    } else {
        "FlowDir"
    }
}

/// `%APPDATA%\FlowDir` (`FlowDir-dev` for development builds)
pub fn config_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, ApiError> {
    app.path()
        .data_dir()
        .map(|dir| dir.join(dir_name(app)))
        .map_err(|e| ApiError::new("CONFIG_DIR", e.to_string()))
}

/// `%LOCALAPPDATA%\FlowDir` (`FlowDir-dev` for development builds)
pub fn cache_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, ApiError> {
    app.path()
        .local_data_dir()
        .map(|dir| dir.join(dir_name(app)))
        .map_err(|e| ApiError::new("CONFIG_DIR", e.to_string()))
}

/// Moves settings, layout and the WebView2 profile out of the identifier-derived folder
/// into `%APPDATA%\FlowDir` and `%LOCALAPPDATA%\FlowDir`. Runs before the window exists so
/// the browser profile is not in use; every step is a no-op once migrated.
pub fn migrate_legacy<R: Runtime>(app: &AppHandle<R>) {
    // A dev run must not move the release's pre-migration files out from under it.
    if is_dev(app) {
        return;
    }
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
///
/// The temporary name carries the PID and a sequence number: a fixed `.tmp` let two
/// concurrent writers truncate each other and could install a half-written JSON file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        // Flush before the rename: a power cut must not leave an empty file in place of
        // the previous, valid one.
        file.sync_all()?;
    }
    if path.exists() {
        let backup = path.with_file_name(format!("{name}.bak"));
        let _ = std::fs::copy(path, backup);
    }
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = std::fs::remove_file(&tmp);
            Err(err)
        }
    }
}
