//! Release detection and self-upgrade.
//!
//! Releases are published by `.github/workflows/release.yml` as three assets: the portable
//! single-file executable, the NSIS installer, and `latest.json` with both SHA-256 digests.
//! The repository slug is baked in at build time (`FLOWDIR_UPDATE_REPO`), so a build made
//! without it still runs but reports `UPDATE_SOURCE` instead of checking.
//!
//! Transport is WinHTTP (system proxy/TLS) and hashing is CNG; neither pulls a crate into
//! the binary that already talks to Win32 everywhere else.

use std::fs;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};
use windows::core::PCWSTR;
use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
    WinHttpQueryOption, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetOption, WinHttpSetTimeouts, INTERNET_DEFAULT_HTTPS_PORT,
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_OPTION_REDIRECT_POLICY,
    WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP, WINHTTP_OPTION_URL,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};
use windows::Win32::Security::Cryptography::{
    BCryptCloseAlgorithmProvider, BCryptGenRandom, BCryptHash, BCryptOpenAlgorithmProvider,
    BCRYPT_ALG_HANDLE, BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS, BCRYPT_SHA256_ALGORITHM,
    BCRYPT_USE_SYSTEM_PREFERRED_RNG,
};

use crate::error::ApiError;
use crate::settings;
use crate::store;

const STATE_FILE: &str = "update.json";
const AGENT: &str = concat!("FlowDir/", env!("CARGO_PKG_VERSION"));
/// Used when a local build has no `FLOWDIR_UPDATE_REPO`; the About page still needs a
/// working project link.
const DEFAULT_REPO: &str = "fangweilong/flow-dir";
/// The GitHub API is unauthenticated here (60 requests/hour/IP); the About page reuses
/// this cache instead of asking on every open.
const HISTORY_TTL: u64 = 6 * 3600;

/// `owner/repo`, injected by the release workflow so the binary knows where to look.
fn repo() -> Option<String> {
    let raw = std::env::var("FLOWDIR_UPDATE_REPO")
        .ok()
        .or_else(|| option_env!("FLOWDIR_UPDATE_REPO").map(str::to_string))?;
    let slug = raw
        .trim()
        .trim_end_matches('/')
        .trim_start_matches("https://github.com/")
        .trim_start_matches("github.com/")
        .to_string();
    (slug.matches('/').count() == 1).then_some(slug)
}

fn repo_or_default() -> String {
    repo().unwrap_or_else(|| DEFAULT_REPO.to_string())
}

/// Browser URL for the project, shown in Settings.
pub fn repository_url() -> String {
    format!("https://github.com/{}", repo_or_default())
}

/// The repository owner's profile URL, shown next to the project link.
pub fn profile_url() -> String {
    let slug = repo_or_default();
    let owner = slug.split('/').next().unwrap_or("").to_string();
    format!("https://github.com/{owner}")
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub current: String,
    pub latest: Option<String>,
    pub available: bool,
    /// `portable` for the single-file build, `installer` for the NSIS install.
    pub flavor: String,
    pub notes: Option<String>,
    /// Whether this call actually went to the network, or reused the cached result.
    pub checked: bool,
    /// Unix seconds of the last successful check.
    pub last_check: Option<u64>,
    pub error: Option<ApiError>,
}

/// Progress report emitted during an update download/install.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgress {
    /// "connecting" | "downloading" | "verifying" | "ready"
    pub phase: String,
    pub downloaded: u64,
    pub total: u64,
    pub percent: u8,
    pub bytes_per_sec: u64,
}

/// One published release, as shown in the version history list.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseEntry {
    /// Tag without the leading `v`.
    pub version: String,
    pub name: Option<String>,
    pub notes: Option<String>,
    /// ISO-8601 timestamp from GitHub.
    pub published_at: Option<String>,
    pub url: Option<String>,
    pub prerelease: bool,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct State {
    last_check: u64,
    latest: Option<String>,
    notes: Option<String>,
    history: Vec<ReleaseEntry>,
    /// Unix seconds of the last successful history fetch.
    history_at: u64,
}

#[derive(Deserialize, Clone)]
struct Asset {
    name: String,
    sha256: String,
    size: u64,
}

#[derive(Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    notes: Option<String>,
    portable: Asset,
    installer: Asset,
}

enum Flavor {
    Portable,
    Installer,
}

/// Build shape for logs and the update manifest.
pub fn flavor_name() -> &'static str {
    Flavor::detect().as_str()
}

impl Flavor {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Portable => "portable",
            Self::Installer => "installer",
        }
    }

    /// The NSIS bundle drops `uninstall.exe` next to the app; a portable copy does not.
    fn detect() -> Self {
        let installed = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("uninstall.exe").exists()))
            .unwrap_or(false);
        if installed {
            Self::Installer
        } else {
            Self::Portable
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn due(last_check: u64, interval_hours: u32) -> bool {
    if interval_hours == 0 {
        return false;
    }
    if last_check == 0 {
        return true;
    }
    now_secs() >= last_check.saturating_add(u64::from(interval_hours) * 3600)
}

/// Checks for a release. `force` ignores the configured interval (manual button).
pub fn check<R: Runtime>(app: &AppHandle<R>, force: bool) -> UpdateStatus {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let cached = read_state(app);
    let latest_cached = cached.latest.clone();
    let mut status = UpdateStatus {
        current: current.clone(),
        available: latest_cached
            .as_deref()
            .is_some_and(|latest| is_newer(latest, &current)),
        latest: latest_cached,
        flavor: Flavor::detect().as_str().to_string(),
        notes: cached.notes.clone(),
        checked: false,
        last_check: (cached.last_check > 0).then_some(cached.last_check),
        error: None,
    };

    // Must not latch `LOAD_FAILED`: this runs periodically in the background, and a
    // momentary read failure here would disable saves for the rest of the session.
    let interval = settings::load_unlatched(app).update_interval_hours;
    if !force && !due(cached.last_check, interval) {
        return status;
    }
    let Some(slug) = repo() else {
        status.error = Some(ApiError::new(
            "UPDATE_SOURCE",
            "FLOWDIR_UPDATE_REPO was not set when this build was made",
        ));
        return status;
    };

    status.checked = true;
    match fetch_manifest(&slug) {
        Ok(manifest) => {
            let now = now_secs();
            // Re-read under the lock: the history command writes the same file from another
            // blocking task, and replacing the whole snapshot here would drop its update.
            mutate_state(app, |state| {
                state.last_check = now;
                state.latest = Some(manifest.version.clone());
                state.notes = manifest.notes.clone();
            });
            status.last_check = Some(now);
            status.latest = Some(manifest.version.clone());
            status.notes = manifest.notes;
            status.available = is_newer(&manifest.version, &current);
            if status.available {
                let _ = app.emit("flowdir:update", status.clone());
            }
        }
        Err(err) => status.error = Some(err),
    }
    status
}

/// Release history for the settings page. Cached for [`HISTORY_TTL`]; a failed refresh
/// keeps the previous list because GitHub's unauthenticated API is rate limited.
pub fn history<R: Runtime>(app: &AppHandle<R>, force: bool) -> Result<Vec<ReleaseEntry>, ApiError> {
    let slug = repo_or_default();
    let state = read_state(app);
    if !force
        && !state.history.is_empty()
        && now_secs() < state.history_at.saturating_add(HISTORY_TTL)
    {
        return Ok(state.history);
    }
    match fetch_history(&slug) {
        Ok(list) => {
            let refreshed = list.clone();
            mutate_state(app, |next| {
                next.history = refreshed;
                next.history_at = now_secs();
            });
            Ok(list)
        }
        Err(err) if !state.history.is_empty() => {
            log::warn!(
                target: "flowdir::update",
                "update_history failed, keeping cached list: {} {}",
                err.code,
                err.detail
            );
            Ok(state.history)
        }
        Err(err) => Err(err),
    }
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}

fn fetch_history(slug: &str) -> Result<Vec<ReleaseEntry>, ApiError> {
    let path = format!("/repos/{slug}/releases?per_page=30");
    let bytes = http_get("api.github.com", &path)?;
    parse_releases(&bytes)
}

fn parse_releases(bytes: &[u8]) -> Result<Vec<ReleaseEntry>, ApiError> {
    let releases: Vec<GhRelease> = serde_json::from_slice(bytes)
        .map_err(|e| ApiError::new("UPDATE_HISTORY", e.to_string()))?;
    Ok(releases
        .into_iter()
        .filter(|release| !release.draft)
        .map(|release| ReleaseEntry {
            version: release.tag_name.trim().trim_start_matches('v').to_string(),
            name: release.name.filter(|name| !name.trim().is_empty()),
            notes: release
                .body
                .map(|body| clip(body, 8_000))
                .filter(|body| !body.trim().is_empty()),
            published_at: release.published_at,
            url: release.html_url,
            prerelease: release.prerelease,
        })
        .collect())
}

/// A release body is arbitrary markdown; bound what reaches the WebView.
fn clip(text: String, max: usize) -> String {
    if text.chars().count() <= max {
        return text;
    }
    let mut clipped: String = text.chars().take(max).collect();
    clipped.push('…');
    clipped
}

/// Downloads the asset for the running flavor, verifies its digest, swaps it in, and
/// relaunches through a detached helper that waits for this process to exit.
/// Guards the whole install flow. A second click (or a second IPC call) must not replace
/// the portable image twice or launch two helpers.
static INSTALLING: AtomicBool = AtomicBool::new(false);

pub fn install<R: Runtime>(app: &AppHandle<R>) -> Result<(), ApiError> {
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Err(ApiError::new("UPDATE_BUSY", "an update is already being installed"));
    }
    let result = install_inner(app);
    if result.is_err() {
        INSTALLING.store(false, Ordering::SeqCst);
    }
    result
}

fn install_inner<R: Runtime>(app: &AppHandle<R>) -> Result<(), ApiError> {
    // A dev binary sits in target/ and must never be replaced by a release asset.
    if store::is_dev(app) {
        return Err(ApiError::new(
            "UPDATE_DEV",
            "development builds cannot install release updates",
        ));
    }
    let slug = repo()
        .ok_or_else(|| ApiError::new("UPDATE_SOURCE", "release repository is not configured"))?;
    let manifest = fetch_manifest(&slug)?;
    if !is_newer(&manifest.version, env!("CARGO_PKG_VERSION")) {
        return Err(ApiError::new("UPDATE_LATEST", manifest.version));
    }
    let flavor = Flavor::detect();
    let asset = match flavor {
        Flavor::Portable => &manifest.portable,
        Flavor::Installer => &manifest.installer,
    };
    let nonce = random_hex()
        .ok_or_else(|| ApiError::new("UPDATE_IO", "secure random source unavailable"))?;
    let dir = std::env::temp_dir().join(format!("FlowDir-update-{nonce}"));
    fs::create_dir_all(&dir).map_err(|e| ApiError::new("UPDATE_IO", e.to_string()))?;
    let downloaded = dir.join(&asset.name);
    download(app, &slug, asset, &downloaded)?;

    let exe = std::env::current_exe().map_err(|e| ApiError::new("UPDATE_IO", e.to_string()))?;
    let mut staged: Option<PathBuf> = None;
    let mut envs: Vec<(&str, PathBuf)> = vec![(ENV_EXE, exe.clone())];
    let body = match flavor {
        Flavor::Portable => {
            // Re-hash right before staging so a file swapped after the download (TOCTOU)
            // is rejected.
            verify_file(&downloaded, asset)?;
            // The staged copy is written now; the swap runs in the helper, after this
            // process is gone. Writing the live image here would leave a half-copied
            // executable behind if the shutdown were interrupted.
            let path = stage_portable(&exe, &downloaded)?;
            let _ = fs::remove_file(&downloaded);
            envs.push((ENV_OLD, exe.with_extension("old.exe")));
            envs.push((ENV_STAGED, path.clone()));
            staged = Some(path);
            portable_body()
        }
        Flavor::Installer => {
            verify_file(&downloaded, asset)?;
            // The setup runs from `%TEMP%`, which any process of this user can write, and
            // UAC is only requested when the helper reaches `start`. Re-hashing inside the
            // helper collapses that TOCTOU window to the gap between `certutil` and
            // `start`. A same-user attacker already has code execution, so this is
            // hardening, not a complete boundary: closing it fully needs an elevated
            // verifier that hashes and then launches the setup itself.
            envs.push((ENV_SETUP, downloaded));
            let digest = asset.sha256.to_ascii_uppercase();
            format!(
                "certutil -hashfile \"%{ENV_SETUP}%\" SHA256 | find /I \"{digest}\" >nul\r\n\
                 if errorlevel 1 goto launch\r\n\
                 start \"\" /wait \"%{ENV_SETUP}%\" /S\r\n\
                 :launch\r\n\
                 start \"\" \"%{ENV_EXE}%\""
            )
        }
    };
    let env_refs: Vec<(&str, &Path)> = envs.iter().map(|(key, path)| (*key, path.as_path())).collect();
    if let Err(err) = spawn_helper(&body, &env_refs) {
        // The live image is untouched; only the staged copy has to go.
        if let Some(staged) = staged {
            let _ = fs::remove_file(staged);
        }
        return Err(err);
    }

    let handle = app.clone();
    std::thread::spawn(move || {
        // Give the IPC reply and the helper a moment; the helper polls until we are gone.
        std::thread::sleep(Duration::from_millis(600));
        // Same exit path as the tray: flush layout and let running copies wind down first,
        // otherwise the installer could start while this process still holds the image.
        crate::shutdown(&handle, false);
    });
    Ok(())
}

/// Removes the `*.old.exe` left behind by a portable self-update and any `*.new.exe` a
/// helper never managed to move into place. Fails harmlessly while an image is still mapped;
/// the next launch retries.
pub fn cleanup_leftovers() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(exe.with_extension("old.exe"));
        let _ = fs::remove_file(exe.with_extension("new.exe"));
    }
    // Every update downloads into its own scratch folder; the installer flavor leaves a
    // full setup there. Only old folders are removed: a second copy of the app (dev and
    // release can run together) may be updating right now.
    let now = SystemTime::now();
    if let Ok(entries) = fs::read_dir(std::env::temp_dir()) {
        for entry in entries.flatten() {
            let modified = entry.metadata().ok().and_then(|meta| meta.modified().ok());
            if stale_update_dir(&entry.file_name(), modified, now) {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
}

const UPDATE_SCRATCH_PREFIX: &str = "FlowDir-update-";
/// Long enough that an update in progress is never touched, short enough to bound `%TEMP%`.
const UPDATE_SCRATCH_TTL: Duration = Duration::from_secs(30 * 60);

fn stale_update_dir(
    name: &std::ffi::OsStr,
    modified: Option<SystemTime>,
    now: SystemTime,
) -> bool {
    name.to_string_lossy().starts_with(UPDATE_SCRATCH_PREFIX)
        && modified.is_some_and(|time| {
            now.duration_since(time)
                .is_ok_and(|age| age >= UPDATE_SCRATCH_TTL)
        })
}

/// Background checker: runs once shortly after launch, then wakes every 30 minutes and
/// lets [`due`] decide whether the configured interval has elapsed.
pub fn spawn_scheduler<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(20));
        loop {
            if repo().is_some() {
                let _ = check(&app, false);
            }
            std::thread::sleep(Duration::from_secs(30 * 60));
        }
    });
}

fn read_state<R: Runtime>(app: &AppHandle<R>) -> State {
    store::config_dir(app)
        .ok()
        .and_then(|dir| fs::read(dir.join(STATE_FILE)).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Serialises read-modify-write cycles on `update.json`. `check` and `history` are separate
/// blocking commands, so an unlocked whole-file write from one would silently revert the
/// other's fields.
static STATE_LOCK: Mutex<()> = Mutex::new(());

fn mutate_state<R: Runtime>(app: &AppHandle<R>, mutate: impl FnOnce(&mut State)) {
    let _guard = STATE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut state = read_state(app);
    mutate(&mut state);
    write_state(app, &state);
}

fn write_state<R: Runtime>(app: &AppHandle<R>, state: &State) {
    let Ok(dir) = store::config_dir(app) else {
        return;
    };
    let _ = fs::create_dir_all(&dir);
    if let Ok(bytes) = serde_json::to_vec_pretty(state) {
        let _ = store::write_atomic(&dir.join(STATE_FILE), &bytes);
    }
}

fn fetch_manifest(slug: &str) -> Result<Manifest, ApiError> {
    let path = format!("/{slug}/releases/latest/download/latest.json");
    let bytes = http_get("github.com", &path)?;
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|e| ApiError::new("UPDATE_MANIFEST", e.to_string()))?;
    validate_asset(&manifest.portable)?;
    validate_asset(&manifest.installer)?;
    Ok(manifest)
}

/// Both digest and size are mandatory now: a manifest served from the same (possibly
/// compromised) release cannot vouch for itself, so a missing field is a hard failure.
fn validate_asset(asset: &Asset) -> Result<(), ApiError> {
    if asset.name.is_empty()
        || asset.name.contains('/')
        || asset.name.contains('\\')
        || asset.name.contains(':')
        || asset.name.contains("..")
    {
        return Err(ApiError::new("UPDATE_MANIFEST", asset.name.clone()));
    }
    let digest_ok = asset.sha256.len() == 64 && asset.sha256.bytes().all(|b| b.is_ascii_hexdigit());
    if !digest_ok {
        return Err(ApiError::new(
            "UPDATE_MANIFEST",
            format!("invalid sha256 for {}", asset.name),
        ));
    }
    if asset.size == 0 {
        return Err(ApiError::new(
            "UPDATE_MANIFEST",
            format!("missing size for {}", asset.name),
        ));
    }
    Ok(())
}

fn download<R: Runtime>(
    app: &AppHandle<R>,
    slug: &str,
    asset: &Asset,
    dest: &PathBuf,
) -> Result<(), ApiError> {
    validate_asset(asset)?;
    let emit = |p: UpdateProgress| {
        let _ = app.emit("flowdir:update-progress", p);
    };

    let path = format!("/{slug}/releases/latest/download/{}", asset.name);
    let mut last_err = None;
    for attempt in 1..=3 {
        emit(UpdateProgress {
            phase: if attempt == 1 {
                "connecting".into()
            } else {
                format!("retrying ({attempt}/3)")
            },
            downloaded: 0,
            total: asset.size,
            percent: 0,
            bytes_per_sec: 0,
        });

        let res = http_download_file("github.com", &path, dest, asset.size, |downloaded, speed| {
            let percent = if asset.size > 0 {
                ((downloaded as f64 / asset.size as f64) * 100.0).min(100.0) as u8
            } else {
                0
            };
            emit(UpdateProgress {
                phase: "downloading".into(),
                downloaded,
                total: asset.size,
                percent,
                bytes_per_sec: speed,
            });
        });

        match res {
            Ok(()) => {
                emit(UpdateProgress {
                    phase: "verifying".into(),
                    downloaded: asset.size,
                    total: asset.size,
                    percent: 100,
                    bytes_per_sec: 0,
                });
                match verify_file(dest, asset) {
                    Ok(()) => {
                        last_err = None;
                        break;
                    }
                    Err(e) => {
                        let _ = fs::remove_file(dest);
                        last_err = Some(e);
                    }
                }
            }
            Err(e) => {
                let _ = fs::remove_file(dest);
                last_err = Some(e);
            }
        }
        if attempt < 3 {
            std::thread::sleep(Duration::from_millis(1500));
        }
    }

    if let Some(err) = last_err {
        return Err(err);
    }

    emit(UpdateProgress {
        phase: "ready".into(),
        downloaded: asset.size,
        total: asset.size,
        percent: 100,
        bytes_per_sec: 0,
    });

    Ok(())
}

fn http_download_file<F>(
    host: &str,
    path: &str,
    dest: &Path,
    total_size: u64,
    mut on_progress: F,
) -> Result<(), ApiError>
where
    F: FnMut(u64, u64),
{
    use std::io::Write;
    let mut file = std::fs::File::create(dest).map_err(|e| ApiError::new("UPDATE_IO", e.to_string()))?;

    unsafe {
        let agent = wide(AGENT);
        let session = WinHttpOpen(
            PCWSTR(agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        );
        if session.is_null() {
            return Err(ApiError::new("UPDATE_NET", "WinHttpOpen failed"));
        }
        let session = Handle(session);
        let _ = WinHttpSetTimeouts(session.0, 10_000, 15_000, 15_000, 30_000);

        let host_wide = wide(host);
        let connect = WinHttpConnect(
            session.0,
            PCWSTR(host_wide.as_ptr()),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        );
        if connect.is_null() {
            return Err(ApiError::new(
                "UPDATE_NET",
                format!("cannot connect to {host}"),
            ));
        }
        let connect = Handle(connect);

        let verb = wide("GET");
        let target = wide(path);
        let request = WinHttpOpenRequest(
            connect.0,
            PCWSTR(verb.as_ptr()),
            PCWSTR(target.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        );
        if request.is_null() {
            return Err(ApiError::new("UPDATE_NET", "WinHttpOpenRequest failed"));
        }
        let request = Handle(request);

        let policy: u32 = WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP;
        let policy_bytes =
            std::slice::from_raw_parts(&policy as *const u32 as *const u8, size_of::<u32>());
        let _ = WinHttpSetOption(
            Some(request.0 as *const _),
            WINHTTP_OPTION_REDIRECT_POLICY,
            Some(policy_bytes),
        );

        let headers = wide(&format!(
            "User-Agent: {AGENT}\r\nAccept: application/octet-stream\r\nCache-Control: no-cache"
        ));
        WinHttpSendRequest(
            request.0,
            Some(&headers[..headers.len() - 1]),
            None,
            0,
            0,
            0,
        )
        .map_err(|e| ApiError::new("UPDATE_NET", e.to_string()))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())
            .map_err(|e| ApiError::new("UPDATE_NET", e.to_string()))?;

        let mut status = 0u32;
        let mut len = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut std::ffi::c_void),
            &mut len,
            std::ptr::null_mut(),
        )
        .map_err(|e| ApiError::new("UPDATE_NET", e.to_string()))?;
        if status != 200 {
            return Err(ApiError::new(
                "UPDATE_HTTP",
                format!("status {status} for {path}"),
            ));
        }
        let final_url = query_url(request.0).unwrap_or_default();
        validate_download_url(&final_url)?;

        let mut downloaded = 0u64;
        let mut buffer = vec![0u8; 64 * 1024];
        let mut last_emit = std::time::Instant::now();
        let mut last_bytes = 0u64;

        loop {
            let mut read = 0u32;
            WinHttpReadData(
                request.0,
                buffer.as_mut_ptr() as *mut _,
                buffer.len() as u32,
                &mut read,
            )
            .map_err(|e| ApiError::new("UPDATE_NET", e.to_string()))?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read as usize])
                .map_err(|e| ApiError::new("UPDATE_IO", e.to_string()))?;
            downloaded += read as u64;

            let now = std::time::Instant::now();
            let elapsed = now.duration_since(last_emit);
            if elapsed.as_millis() >= 150 || downloaded == total_size {
                let speed = if elapsed.as_secs_f64() > 0.0 {
                    ((downloaded.saturating_sub(last_bytes)) as f64 / elapsed.as_secs_f64()) as u64
                } else {
                    0
                };
                last_emit = now;
                last_bytes = downloaded;
                on_progress(downloaded, speed);
            }
        }
        file.flush().map_err(|e| ApiError::new("UPDATE_IO", e.to_string()))?;
        if downloaded != total_size {
            return Err(ApiError::new(
                "UPDATE_SIZE",
                format!("{downloaded} != {total_size}"),
            ));
        }
        Ok(())
    }
}

fn verify_file(path: &Path, asset: &Asset) -> Result<(), ApiError> {
    let bytes = fs::read(path).map_err(|e| ApiError::new("UPDATE_IO", e.to_string()))?;
    if bytes.len() as u64 != asset.size {
        return Err(ApiError::new(
            "UPDATE_SIZE",
            format!("{} != {}", bytes.len(), asset.size),
        ));
    }
    if !bytes.starts_with(b"MZ") {
        return Err(ApiError::new("UPDATE_ASSET", "not a PE image"));
    }
    let actual = sha256_hex(&bytes);
    if !actual.eq_ignore_ascii_case(&asset.sha256) {
        return Err(ApiError::new(
            "UPDATE_HASH",
            format!("{actual} != {}", asset.sha256),
        ));
    }
    Ok(())
}

/// Copies the verified download next to the running image as `*.new.exe` and returns that
/// path. The live image is never touched from inside the process: the helper renames it
/// once this process has exited, so an interrupted shutdown cannot leave a truncated exe.
fn stage_portable(exe: &Path, downloaded: &Path) -> Result<PathBuf, ApiError> {
    let staged = exe.with_extension("new.exe");
    let _ = fs::remove_file(&staged);
    fs::copy(downloaded, &staged).map_err(|e| ApiError::new("UPDATE_SWAP", e.to_string()))?;
    Ok(staged)
}

/// Environment variables that carry the paths into the helper. The batch body must stay
/// pure ASCII: `cmd.exe` decodes a `.cmd` file with the console/OEM code page, so a UTF-8
/// script containing `C:\Users\张三\…` becomes mojibake and every `move`/`start` fails.
/// `Command::env` passes Unicode through `CreateProcessW`, and `%VAR%` expansion is
/// Unicode inside cmd, so the paths survive regardless of the active code page.
const ENV_EXE: &str = "FLOWDIR_UPDATE_EXE";
const ENV_OLD: &str = "FLOWDIR_UPDATE_OLD";
const ENV_STAGED: &str = "FLOWDIR_UPDATE_STAGED";
const ENV_SETUP: &str = "FLOWDIR_UPDATE_SETUP";

/// Body of the relaunch helper for a portable update: move the current image aside, move the
/// staged copy into its place, and start it. Every `move` is checked, because the running
/// image may still be locked for a moment after the PID disappears; the previous image is
/// restored in that case and always remains launchable.
fn portable_body() -> String {
    // CRLF matters here: `goto` searches forward for the label and a bare LF can hide it
    // from the command processor.
    format!(
        "if exist \"%{ENV_OLD}%\" del /f /q \"%{ENV_OLD}%\"\r\nmove /Y \"%{ENV_EXE}%\" \"%{ENV_OLD}%\"\r\nif errorlevel 1 goto launch\r\nmove /Y \"%{ENV_STAGED}%\" \"%{ENV_EXE}%\"\r\nif errorlevel 1 move /Y \"%{ENV_OLD}%\" \"%{ENV_EXE}%\"\r\n:launch\r\nstart \"\" \"%{ENV_EXE}%\""
    )
}

/// Writes a small batch helper that polls for this PID and only then starts the new
/// executable (or runs the installer), avoiding a race with the single-instance plugin.
///
/// The probe has to match the PID column: `tasklist` truncates the image name to 24
/// characters, so a released name like `FlowDir_0.6.20_x64_portable.exe` never yields a
/// line ending in `.exe` and the helper would start the new image while this process is
/// still alive, only for the single-instance plugin to forward and exit.
fn spawn_helper(body: &str, envs: &[(&str, &Path)]) -> Result<(), ApiError> {
    let pid = std::process::id();
    let script = std::env::temp_dir().join(format!("flowdir-update-{pid}.cmd"));
    let text = format!(
        "@echo off\r\nsetlocal\r\n:wait\r\ntasklist /FI \"PID eq {pid}\" /NH | find /I \"{pid}\" >nul\r\nif not errorlevel 1 (\r\nping -n 2 127.0.0.1 >nul\r\ngoto wait\r\n)\r\n{body}\r\ndel \"%~f0\"\r\n",
    );
    fs::write(&script, text).map_err(|e| ApiError::new("UPDATE_IO", e.to_string()))?;
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = std::process::Command::new("cmd");
    command.args(["/C", &script.to_string_lossy()]);
    for (key, value) in envs {
        command.env(key, value.as_os_str());
    }
    command
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| ApiError::new("UPDATE_LAUNCH", e.to_string()))?;
    Ok(())
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

struct Handle(*mut std::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

fn http_get(host: &str, path: &str) -> Result<Vec<u8>, ApiError> {
    unsafe {
        let agent = wide(AGENT);
        let session = WinHttpOpen(
            PCWSTR(agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        );
        if session.is_null() {
            return Err(ApiError::new("UPDATE_NET", "WinHttpOpen failed"));
        }
        let session = Handle(session);
        let _ = WinHttpSetTimeouts(session.0, 10_000, 15_000, 15_000, 30_000);

        let host_wide = wide(host);
        let connect = WinHttpConnect(
            session.0,
            PCWSTR(host_wide.as_ptr()),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        );
        if connect.is_null() {
            return Err(ApiError::new(
                "UPDATE_NET",
                format!("cannot connect to {host}"),
            ));
        }
        let connect = Handle(connect);

        let verb = wide("GET");
        let target = wide(path);
        let request = WinHttpOpenRequest(
            connect.0,
            PCWSTR(verb.as_ptr()),
            PCWSTR(target.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        );
        if request.is_null() {
            return Err(ApiError::new("UPDATE_NET", "WinHttpOpenRequest failed"));
        }
        let request = Handle(request);

        let policy: u32 = WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP;
        let policy_bytes =
            std::slice::from_raw_parts(&policy as *const u32 as *const u8, size_of::<u32>());
        let _ = WinHttpSetOption(
            Some(request.0 as *const _),
            WINHTTP_OPTION_REDIRECT_POLICY,
            Some(policy_bytes),
        );

        let headers = wide(&format!(
            "User-Agent: {AGENT}\r\nAccept: application/json\r\nCache-Control: no-cache"
        ));
        WinHttpSendRequest(
            request.0,
            Some(&headers[..headers.len() - 1]),
            None,
            0,
            0,
            0,
        )
        .map_err(|e| ApiError::new("UPDATE_NET", e.to_string()))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())
            .map_err(|e| ApiError::new("UPDATE_NET", e.to_string()))?;

        let mut status = 0u32;
        let mut len = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut std::ffi::c_void),
            &mut len,
            std::ptr::null_mut(),
        )
        .map_err(|e| ApiError::new("UPDATE_NET", e.to_string()))?;
        if status != 200 {
            return Err(ApiError::new(
                "UPDATE_HTTP",
                format!("status {status} for {path}"),
            ));
        }
        let final_url = query_url(request.0).unwrap_or_default();
        validate_download_url(&final_url)?;

        let mut out = Vec::new();
        let mut buffer = vec![0u8; 64 * 1024];
        // Manifests and history are small JSON documents; a hostile/broken endpoint must
        // not be able to exhaust memory before the body is validated.
        const MAX_RESPONSE: usize = 16 * 1024 * 1024;
        loop {
            let mut read = 0u32;
            WinHttpReadData(
                request.0,
                buffer.as_mut_ptr() as *mut _,
                buffer.len() as u32,
                &mut read,
            )
            .map_err(|e| ApiError::new("UPDATE_NET", e.to_string()))?;
            if read == 0 {
                break;
            }
            out.extend_from_slice(&buffer[..read as usize]);
            if out.len() > MAX_RESPONSE {
                return Err(ApiError::new("UPDATE_HTTP", "response body is too large"));
            }
        }
        Ok(out)
    }
}

/// Reads `WINHTTP_OPTION_URL` after the response has been received; at that point it is
/// the URL WinHTTP actually landed on, not the one we asked for.
fn query_url(request: *mut std::ffi::c_void) -> Option<String> {
    unsafe {
        let mut bytes = 0u32;
        let _ = WinHttpQueryOption(request, WINHTTP_OPTION_URL, None, &mut bytes);
        if bytes == 0 {
            return None;
        }
        let mut buffer = vec![0u16; (bytes as usize).div_ceil(2) + 1];
        let mut written = bytes;
        WinHttpQueryOption(
            request,
            WINHTTP_OPTION_URL,
            Some(buffer.as_mut_ptr() as *mut _),
            &mut written,
        )
        .ok()?;
        let text = String::from_utf16_lossy(&buffer);
        Some(text.trim_end_matches('\0').to_string())
    }
}

/// Only HTTPS URLs whose host is GitHub-owned may be downloaded. A 302 to `http://` is
/// already blocked by the redirect policy; this also rejects arbitrary hosts and makes
/// the guarantee explicit for the manifest and asset requests alike.
fn validate_download_url(url: &str) -> Result<(), ApiError> {
    let Some(rest) = url.strip_prefix("https://") else {
        return Err(ApiError::new(
            "UPDATE_SCHEME",
            format!("refusing non-https download: {url}"),
        ));
    };
    // Take the whole authority, not the text before the first `:`: in
    // `https://github.com:pass@evil.example/x` the "host" would otherwise read as
    // `github.com` while the connection target is `evil.example`.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err(ApiError::new(
            "UPDATE_HOST",
            "refusing download URL with userinfo",
        ));
    }
    let host = authority
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let allowed = host == "github.com"
        || host.ends_with(".github.com")
        || host == "githubusercontent.com"
        || host.ends_with(".githubusercontent.com");
    if !allowed {
        return Err(ApiError::new(
            "UPDATE_HOST",
            format!("refusing download from {host}"),
        ));
    }
    Ok(())
}

/// 128-bit random hex, or `None` when the system RNG is unavailable. Falling back to a
/// fixed value would make the staging directory name predictable.
fn random_hex() -> Option<String> {
    let mut bytes = [0u8; 16];
    let generated = unsafe { BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    if generated.is_err() {
        return None;
    }
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn sha256_hex(bytes: &[u8]) -> String {
    unsafe {
        let mut alg = BCRYPT_ALG_HANDLE(std::ptr::null_mut());
        let opened = BCryptOpenAlgorithmProvider(
            &mut alg,
            BCRYPT_SHA256_ALGORITHM,
            PCWSTR::null(),
            BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0),
        );
        if opened.0 < 0 {
            return String::new();
        }
        let mut digest = [0u8; 32];
        let hashed = BCryptHash(alg, None, bytes, &mut digest);
        let _ = BCryptCloseAlgorithmProvider(alg, 0);
        if hashed.0 < 0 {
            return String::new();
        }
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }
}

fn parse_version(version: &str) -> (Vec<u64>, Option<&str>) {
    let trimmed = version.trim().trim_start_matches('v');
    let (core, pre) = match trimmed.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (trimmed, None),
    };
    let parts = core
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect();
    (parts, pre)
}

/// True when `latest` supersedes `current`. Release versions win over a pre-release of the
/// same core; pre-release identifiers are compared lexically, which is enough until we
/// actually publish one.
fn is_newer(latest: &str, current: &str) -> bool {
    let (a, a_pre) = parse_version(latest);
    let (b, b_pre) = parse_version(current);
    for index in 0..a.len().max(b.len()) {
        let left = a.get(index).copied().unwrap_or(0);
        let right = b.get(index).copied().unwrap_or(0);
        if left != right {
            return left > right;
        }
    }
    match (a_pre, b_pre) {
        (None, Some(_)) => true,
        (Some(_), None) => false,
        (Some(left), Some(right)) => left > right,
        (None, None) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_old_update_scratch_dirs_are_reclaimed() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000_000);
        let fresh = now - Duration::from_secs(60);
        let old = now - Duration::from_secs(3 * 60 * 60);
        let name = std::ffi::OsStr::new("FlowDir-update-abc123");
        assert!(stale_update_dir(name, Some(old), now));
        assert!(!stale_update_dir(name, Some(fresh), now));
        // A live download has a fresh mtime; a missing one or a foreign name is kept.
        assert!(!stale_update_dir(name, None, now));
        assert!(!stale_update_dir(
            std::ffi::OsStr::new("FlowDir-update"),
            Some(old),
            now
        ));
    }

    #[test]
    fn portable_stage_never_touches_the_running_image() {
        let dir = std::env::temp_dir().join(format!("flowdir-stage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("FlowDir.exe");
        let downloaded = dir.join("new.exe");
        fs::write(&exe, b"old").unwrap();
        fs::write(&downloaded, b"new").unwrap();
        let staged = stage_portable(&exe, &downloaded).unwrap();
        assert_eq!(staged, dir.join("FlowDir.new.exe"));
        assert_eq!(fs::read(&staged).unwrap(), b"new");
        // The live image is untouched until the helper runs after exit.
        assert_eq!(fs::read(&exe).unwrap(), b"old");
        // Staging again replaces a leftover instead of appending to it.
        fs::write(&staged, b"stale-bytes").unwrap();
        let staged = stage_portable(&exe, &downloaded).unwrap();
        assert_eq!(fs::read(&staged).unwrap(), b"new");
        let body = portable_body();
        assert!(body.starts_with(&format!("if exist \"%{ENV_OLD}%\"")));
        assert!(body.contains(&format!("move /Y \"%{ENV_EXE}%\"")));
        assert!(body.contains(":launch"));
        assert!(body.ends_with(&format!("start \"\" \"%{ENV_EXE}%\"")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn helper_script_stays_ascii_for_non_ascii_paths() {
        // `cmd.exe` decodes a .cmd file with the OEM code page; embedding the Unicode
        // install path in the script body garbles it. Every path must arrive through the
        // environment, so the body is pure ASCII.
        assert!(portable_body().is_ascii());
    }

    #[test]
    fn newer_versions_win() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(is_newer("1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.1"));
        assert!(!is_newer("0.1.0-rc1", "0.1.0"));
        assert!(is_newer("0.1.0", "0.1.0-rc1"));
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn manifest_reads_both_flavors() {
        let json = r#"{"version":"0.2.0","portable":{"name":"FlowDir_0.2.0_x64_portable.exe","sha256":"aa","size":10},"installer":{"name":"FlowDir_0.2.0_x64-setup.exe","sha256":"bb","size":20}}"#;
        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.version, "0.2.0");
        assert_eq!(manifest.portable.name, "FlowDir_0.2.0_x64_portable.exe");
        assert_eq!(manifest.installer.size, 20);
    }

    #[test]
    fn interval_gates_checks() {
        assert!(due(0, 24));
        assert!(due(now_secs() - 26 * 3600, 24));
        assert!(!due(now_secs() - 3600, 24));
        assert!(!due(0, 0));
    }

    #[test]
    fn download_url_must_be_https_and_github() {
        assert!(validate_download_url("https://github.com/o/r/releases/x").is_ok());
        assert!(validate_download_url("https://objects.githubusercontent.com/x").is_ok());
        assert!(validate_download_url("http://github.com/o/r").is_err());
        assert!(validate_download_url("https://evil.example.com/x").is_err());
        assert!(validate_download_url("https://github.com.evil.example/x").is_err());
        // Userinfo must not smuggle another connection target past the host check.
        assert!(validate_download_url("https://github.com:pass@evil.example/x").is_err());
        assert!(validate_download_url("https://user@evil.example/o/r").is_err());
        // A port after the allowed host is still fine.
        assert!(validate_download_url("https://github.com:443/o/r").is_ok());
    }

    #[test]
    fn assets_require_digest_and_size() {
        let ok = Asset {
            name: "a.exe".into(),
            sha256: "a".repeat(64),
            size: 1,
        };
        assert!(validate_asset(&ok).is_ok());
        let no_digest = Asset {
            sha256: String::new(),
            ..ok.clone()
        };
        assert!(validate_asset(&no_digest).is_err());
        let no_size = Asset { size: 0, ..ok.clone() };
        assert!(validate_asset(&no_size).is_err());
        let traversal = Asset {
            name: "../a.exe".into(),
            ..ok
        };
        assert!(validate_asset(&traversal).is_err());
    }

    #[test]
    fn history_skips_drafts_and_strips_v_prefix() {
        let json = br#"[
            {"tag_name":"v0.6.29","name":"v0.6.29","body":"Changes\n- item","published_at":"2026-11-01T00:00:00Z","html_url":"https://github.com/o/r/releases/tag/v0.6.29","prerelease":false,"draft":false},
            {"tag_name":"v0.7.0-rc1","name":"","body":"","published_at":null,"html_url":null,"prerelease":true,"draft":false},
            {"tag_name":"v0.8.0","name":"draft","body":null,"published_at":null,"html_url":null,"prerelease":false,"draft":true}
        ]"#;
        let releases = parse_releases(json).unwrap();
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].version, "0.6.29");
        assert_eq!(releases[0].notes.as_deref(), Some("Changes\n- item"));
        assert_eq!(releases[1].version, "0.7.0-rc1");
        assert_eq!(releases[1].name, None);
        assert_eq!(releases[1].notes, None);
        assert!(releases[1].prerelease);
    }

    #[test]
    fn release_notes_are_clipped_on_char_boundaries() {
        let text = "更".repeat(10);
        let clipped = clip(text.clone(), 4);
        assert_eq!(clipped, "更更更更…");
        assert_eq!(clip(text, 10).chars().count(), 10);
    }

    #[test]
    fn repository_links_are_browsable() {
        assert_eq!(repository_url(), "https://github.com/fangweilong/flow-dir");
        assert_eq!(profile_url(), "https://github.com/fangweilong");
    }
}
