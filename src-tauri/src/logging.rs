//! Rolling diagnostic log for the backend, plus the diagnostics bundle.
//!
//! `log` is the facade; the writer below owns rotation so the file never grows without
//! bound. Paths stay under `%LOCALAPPDATA%\FlowDir(-dev)\logs`, beside the WebView2 cache,
//! and the log is intentionally append-only and human-readable: it exists to answer
//! "what did the app do?" from a user's machine.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use log::{LevelFilter, Log, Metadata, Record};
use serde_json::json;
use tauri::{AppHandle, Runtime};

use crate::error::ApiError;
use crate::settings::Settings;

/// Per-file cap; five rotations bound the folder at ~12 MB.
const MAX_BYTES: u64 = 2 * 1024 * 1024;
const KEEP: usize = 5;
const LOG_FILE: &str = "flowdir.log";

struct Logger {
    state: Mutex<State>,
}

struct State {
    dir: PathBuf,
    file: File,
    size: u64,
    level: LevelFilter,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();
static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn filter(level: &str) -> LevelFilter {
    match level {
        "error" => LevelFilter::Error,
        "warn" => LevelFilter::Warn,
        "debug" | "trace" => LevelFilter::Debug,
        _ => LevelFilter::Info,
    }
}

impl State {
    fn open(dir: PathBuf) -> Result<Self, ApiError> {
        fs::create_dir_all(&dir).map_err(|e| ApiError::from_io(&dir.to_string_lossy(), &e))?;
        // Leftovers from an older cap must not accumulate forever.
        for index in (KEEP + 1)..=(KEEP + 8) {
            let _ = fs::remove_file(dir.join(format!("flowdir.{index}.log")));
        }
        let path = dir.join(LOG_FILE);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| ApiError::from_io(&path.to_string_lossy(), &e))?;
        let size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        Ok(Self {
            dir,
            file,
            size,
            level: LevelFilter::Info,
        })
    }

    /// Shifts `flowdir.log` → `.1`, `.1` → `.2`, … and drops the oldest rotation.
    fn rotate(&mut self) {
        for index in (1..KEEP).rev() {
            let from = self.dir.join(format!("flowdir.{index}.log"));
            let to = self.dir.join(format!("flowdir.{}.log", index + 1));
            let _ = fs::rename(from, to);
        }
        let older = self.dir.join(format!("flowdir.{}.log", KEEP + 1));
        let _ = fs::remove_file(older);
        let current = self.dir.join(LOG_FILE);
        let _ = fs::rename(&current, self.dir.join("flowdir.1.log"));
        if let Ok(file) = OpenOptions::new().create(true).append(true).open(&current) {
            self.file = file;
        }
        self.size = 0;
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.state.lock().unwrap().level
    }

    fn log(&self, record: &Record) {
        let mut state = self.state.lock().unwrap();
        if record.level() > state.level {
            return;
        }
        let line = format!(
            "[{}] {:<5} {}: {}\n",
            timestamp(),
            record.level(),
            record.target(),
            record.args()
        );
        if state.size + line.len() as u64 > MAX_BYTES {
            state.rotate();
        }
        if state.file.write_all(line.as_bytes()).is_ok() {
            state.size += line.len() as u64;
        }
    }

    fn flush(&self) {
        let _ = self.state.lock().unwrap().file.flush();
    }
}

/// Opens the log for this process. Called once from `setup`, before anything logs.
pub fn init(dir: PathBuf, level: &str) -> Result<(), ApiError> {
    if LOGGER.get().is_some() {
        set_level(level);
        return Ok(());
    }
    let state = Mutex::new(State::open(dir)?);
    if LOGGER.set(Logger { state }).is_err() {
        set_level(level);
        return Ok(());
    }
    let logger = LOGGER.get().expect("the logger was just set");
    logger.state.lock().unwrap().level = filter(level);
    log::set_max_level(filter(level));
    let _ = log::set_logger(logger);
    Ok(())
}

/// Applies the level from settings at runtime.
pub fn set_level(level: &str) {
    let filter = filter(level);
    log::set_max_level(filter);
    if let Some(logger) = LOGGER.get() {
        logger.state.lock().unwrap().level = filter;
    }
}

pub fn dir() -> Option<PathBuf> {
    LOGGER
        .get()
        .map(|logger| logger.state.lock().unwrap().dir.clone())
}

/// Logs a failure at most once per key per process, so a broken watcher or a folder that
/// is scanned over and over cannot flood the file.
pub fn error_once(key: &str, message: &str) {
    let seen = SEEN.get_or_init(|| Mutex::new(HashSet::new()));
    {
        let mut seen = seen.lock().unwrap();
        if !seen.insert(key.to_string()) {
            return;
        }
        if seen.len() > 512 {
            seen.clear();
        }
    }
    log::error!("{message}");
}

/// Frontend crash/error reports share the same file and are deduped before they get here.
pub fn frontend(level: &str, message: &str, detail: &str) {
    let message: String = message.chars().take(2_000).collect();
    let detail: String = detail.chars().take(4_000).collect();
    match level {
        "error" => log::error!(target: "flowdir::frontend", "{message} {detail}"),
        "warn" => log::warn!(target: "flowdir::frontend", "{message} {detail}"),
        "debug" => log::debug!(target: "flowdir::frontend", "{message} {detail}"),
        _ => log::info!(target: "flowdir::frontend", "{message} {detail}"),
    }
}

fn payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// Records panics with their location and a backtrace; release builds keep the hook.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|spot| format!("{}:{}:{}", spot.file(), spot.line(), spot.column()))
            .unwrap_or_else(|| "unknown location".into());
        let message = payload_message(info.payload());
        let backtrace = std::backtrace::Backtrace::force_capture();
        log::error!("panic at {location}: {message}\n{backtrace}");
    }));
}

/// Startup banner: the facts needed to reproduce a report without asking the user.
pub fn log_startup<R: Runtime>(app: &AppHandle<R>) {
    let config = crate::store::config_dir(app)
        .map(|dir| dir.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unavailable".into());
    log::info!(
        "FlowDir {} ({}) starting: windows={} webview2={} config={} logdir={}",
        env!("CARGO_PKG_VERSION"),
        flavor(app),
        windows_version(),
        webview2_version(),
        config,
        dir().map(|dir| dir.to_string_lossy().into_owned()).unwrap_or_default(),
    );
}

fn flavor<R: Runtime>(app: &AppHandle<R>) -> &'static str {
    if crate::store::is_dev(app) {
        "dev"
    } else {
        crate::update::flavor_name()
    }
}

fn reg_string(subkey: &str, value: &str) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

    let subkey: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    let value: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buffer = [0u16; 512];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr() as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    if status.is_err() {
        return None;
    }
    let len = buffer.iter().position(|unit| *unit == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&buffer[..len]))
}

/// `ProductName` plus the build number, which is what support actually needs.
pub fn windows_version() -> String {
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let name = reg_string(KEY, "ProductName").unwrap_or_else(|| "Windows".into());
    match reg_string(KEY, "CurrentBuild") {
        Some(build) => format!("{name} (build {build})"),
        None => name,
    }
}

/// Version of the installed WebView2 runtime, or `unknown` when it cannot be read.
pub fn webview2_version() -> String {
    const KEY: &str = r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    reg_string(KEY, "pv").unwrap_or_else(|| "unknown".into())
}

/// Bundles logs, version facts and a redacted settings snapshot into a timestamped zip.
/// Settings that hold user paths (`custom_places`, layout, external commands) are omitted.
pub fn export<R: Runtime>(
    app: &AppHandle<R>,
    settings: &Settings,
    target: &str,
) -> Result<String, ApiError> {
    let log_dir = dir().ok_or_else(|| ApiError::new("NO_LOG", "the log is not initialised"))?;
    let logs = log_files(&log_dir);
    let info = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "flavor": flavor(app),
        "windows": windows_version(),
        "webview2": webview2_version(),
        "configDir": crate::store::config_dir(app).map(|dir| dir.to_string_lossy().into_owned()).ok(),
        "logLevel": settings.log_level,
        "logFiles": logs,
        "exportedAt": timestamp(),
    });
    export_bundle(&log_dir, target, &info, &redacted_settings(settings))
}

fn log_files(log_dir: &Path) -> Vec<String> {
    let mut logs = Vec::new();
    if let Ok(entries) = fs::read_dir(log_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("flowdir") && name.ends_with(".log") {
                logs.push(name);
            }
        }
    }
    logs.sort();
    logs
}

/// Redacted on purpose: no custom places, no layout, no external preview commands.
fn redacted_settings(settings: &Settings) -> serde_json::Value {
    json!({
        "language": settings.language,
        "theme": settings.theme,
        "previewOpen": settings.preview_open,
        "showHidden": settings.show_hidden,
        "useEverything": settings.use_everything,
        "sidebarOpen": settings.sidebar_open,
        "sidebarWidth": settings.sidebar_width,
        "previewWidth": settings.preview_width,
        "gpuAccel": settings.gpu_accel,
        "updateIntervalHours": settings.update_interval_hours,
        "notifyOps": settings.notify_ops,
        "fontFamily": settings.font_family,
        "fontSize": settings.font_size,
        "tabFontSize": settings.tab_font_size,
        "listFontSize": settings.list_font_size,
        "navFontSize": settings.nav_font_size,
        "listSizeWidth": settings.list_size_width,
        "listTimeWidth": settings.list_time_width,
        "logLevel": settings.log_level,
    })
}

/// Writes the zip itself; separated from the app handle so the bundle can be tested.
pub fn export_bundle(
    log_dir: &Path,
    target: &str,
    info: &serde_json::Value,
    settings: &serde_json::Value,
) -> Result<String, ApiError> {
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    let target_dir = Path::new(target);
    fs::create_dir_all(target_dir)
        .map_err(|e| ApiError::from_io(&target_dir.to_string_lossy(), &e))?;
    let stamp = timestamp().replace([':', '.'], "-");
    let path = target_dir.join(format!("FlowDir-diagnostics-{stamp}.zip"));
    let file = File::create(&path).map_err(|e| ApiError::from_io(&path.to_string_lossy(), &e))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    let write = |zip: &mut ZipWriter<File>, name: &str, bytes: &[u8]| -> Result<(), ApiError> {
        zip.start_file(name, options)
            .map_err(|e| ApiError::new("ZIP", e.to_string()))?;
        zip.write_all(bytes)
            .map_err(|e| ApiError::new("ZIP", e.to_string()))
    };

    for name in log_files(log_dir) {
        if let Ok(bytes) = fs::read(log_dir.join(&name)) {
            write(&mut zip, &name, &bytes)?;
        }
    }
    write(&mut zip, "info.json", info.to_string().as_bytes())?;
    write(&mut zip, "settings.json", settings.to_string().as_bytes())?;
    zip.finish().map_err(|e| ApiError::new("ZIP", e.to_string()))?;
    log::info!("diagnostics bundle written to {}", path.display());
    Ok(path.to_string_lossy().into_owned())
}

/// UTC timestamp with milliseconds; log readers compare runs, not local clocks.
fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let millis = now.subsec_millis();
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's days→civil conversion (no chrono dependency).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_is_iso_utc() {
        let stamp = timestamp();
        assert_eq!(stamp.len(), 24, "{stamp}");
        assert!(stamp.ends_with('Z'));
        assert_eq!(&stamp[4..5], "-");
        assert_eq!(&stamp[10..11], "T");
    }

    #[test]
    fn civil_dates_match_known_values() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }

    #[test]
    fn writes_timestamped_lines() {
        let dir = std::env::temp_dir().join(format!("flowdir-logline-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let logger = Logger {
            state: Mutex::new(State::open(dir.clone()).unwrap()),
        };
        let record = Record::builder()
            .args(format_args!("hello world"))
            .level(log::Level::Info)
            .target("flowdir::test")
            .build();
        Log::log(&logger, &record);
        let text = fs::read_to_string(dir.join(LOG_FILE)).unwrap();
        assert!(text.starts_with('['), "{text}");
        assert!(text.contains(" INFO "), "{text}");
        assert!(text.contains("flowdir::test: hello world"), "{text}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_writes_to_the_requested_directory() {
        let log_dir = std::env::temp_dir().join(format!("flowdir-loginit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&log_dir);
        init(log_dir.clone(), "debug").unwrap();
        log::info!("init probe");
        let text = fs::read_to_string(log_dir.join(LOG_FILE)).unwrap();
        assert!(text.contains("init probe"), "{text}");
        assert_eq!(dir(), Some(log_dir.clone()));
        let _ = fs::remove_dir_all(&log_dir);
    }

    #[test]
    fn reports_os_and_webview_versions() {
        assert!(windows_version().contains("Windows"), "{}", windows_version());
        assert!(!webview2_version().is_empty());
    }

    #[test]
    fn bundle_contains_logs_and_settings() {
        let root = std::env::temp_dir().join(format!("flowdir-bundle-{}", std::process::id()));
        let logs = root.join("logs");
        let out = root.join("out");
        fs::create_dir_all(&logs).unwrap();
        fs::write(logs.join(LOG_FILE), b"[t] INFO test: hello\n").unwrap();
        fs::write(logs.join("notes.txt"), b"not a log").unwrap();
        let path = export_bundle(
            &logs,
            &out.to_string_lossy(),
            &json!({ "version": "test" }),
            &json!({ "language": "zh-CN" }),
        )
        .unwrap();
        let mut archive = zip::ZipArchive::new(File::open(&path).unwrap()).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|index| {
                archive
                    .by_index(index)
                    .unwrap()
                    .name()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert!(names.contains(&LOG_FILE.to_string()), "{names:?}");
        assert!(names.contains(&"info.json".to_string()), "{names:?}");
        assert!(names.contains(&"settings.json".to_string()), "{names:?}");
        assert!(!names.contains(&"notes.txt".to_string()), "only logs are bundled");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rotates_and_keeps_the_newest_log() {
        let dir = std::env::temp_dir().join(format!("flowdir-log-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let logger = Logger {
            state: Mutex::new(State::open(dir.clone()).unwrap()),
        };
        {
            let mut state = logger.state.lock().unwrap();
            state.size = MAX_BYTES;
            state.rotate();
        }
        assert!(dir.join("flowdir.log").exists());
        assert!(dir.join("flowdir.1.log").exists());
        {
            let mut state = logger.state.lock().unwrap();
            state.size = MAX_BYTES;
            state.rotate();
        }
        assert!(dir.join("flowdir.2.log").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
