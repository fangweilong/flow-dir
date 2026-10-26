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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};
use windows::core::PCWSTR;
use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption,
    WinHttpSetTimeouts, INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
    WINHTTP_FLAG_SECURE, WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_ALWAYS,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};
use windows::Win32::Security::Cryptography::{
    BCryptCloseAlgorithmProvider, BCryptHash, BCryptOpenAlgorithmProvider, BCRYPT_ALG_HANDLE,
    BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS, BCRYPT_SHA256_ALGORITHM,
};

use crate::error::ApiError;
use crate::settings;
use crate::store;

const STATE_FILE: &str = "update.json";
const AGENT: &str = concat!("FlowDir/", env!("CARGO_PKG_VERSION"));

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

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct State {
    last_check: u64,
    latest: Option<String>,
    notes: Option<String>,
}

#[derive(Deserialize, Clone)]
struct Asset {
    name: String,
    #[serde(default)]
    sha256: String,
    #[serde(default)]
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

    let interval = settings::load(app).update_interval_hours;
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
            write_state(
                app,
                &State {
                    last_check: now,
                    latest: Some(manifest.version.clone()),
                    notes: manifest.notes.clone(),
                },
            );
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

/// Downloads the asset for the running flavor, verifies its digest, swaps it in, and
/// relaunches through a detached helper that waits for this process to exit.
pub fn install<R: Runtime>(app: &AppHandle<R>) -> Result<(), ApiError> {
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
    let dir = std::env::temp_dir().join("FlowDir-update");
    fs::create_dir_all(&dir).map_err(|e| ApiError::new("UPDATE_IO", e.to_string()))?;
    let downloaded = dir.join(&asset.name);
    download(&slug, asset, &downloaded)?;

    let exe = std::env::current_exe().map_err(|e| ApiError::new("UPDATE_IO", e.to_string()))?;
    let body = match flavor {
        Flavor::Portable => {
            replace_portable(&exe, &downloaded)?;
            let _ = fs::remove_file(&downloaded);
            format!("start \"\" \"{}\"", exe.display())
        }
        Flavor::Installer => format!(
            "start \"\" /wait \"{}\" /S\nstart \"\" \"{}\"",
            downloaded.display(),
            exe.display()
        ),
    };
    spawn_helper(&body)?;

    let handle = app.clone();
    std::thread::spawn(move || {
        // Give the IPC reply and the helper a moment; the helper polls until we are gone.
        std::thread::sleep(Duration::from_millis(600));
        handle.exit(0);
    });
    Ok(())
}

/// Removes the `*.old.exe` left behind by a portable self-update. Fails harmlessly while
/// the previous image is still mapped; the next launch retries.
pub fn cleanup_leftovers() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(exe.with_extension("old.exe"));
    }
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
    serde_json::from_slice(&bytes).map_err(|e| ApiError::new("UPDATE_MANIFEST", e.to_string()))
}

fn download(slug: &str, asset: &Asset, dest: &PathBuf) -> Result<(), ApiError> {
    if asset.name.is_empty() || asset.name.contains('/') || asset.name.contains('\\') {
        return Err(ApiError::new("UPDATE_MANIFEST", asset.name.clone()));
    }
    let path = format!("/{slug}/releases/latest/download/{}", asset.name);
    let bytes = http_get("github.com", &path)?;
    if asset.size > 0 && bytes.len() as u64 != asset.size {
        return Err(ApiError::new(
            "UPDATE_SIZE",
            format!("{} != {}", bytes.len(), asset.size),
        ));
    }
    if !bytes.starts_with(b"MZ") {
        return Err(ApiError::new("UPDATE_ASSET", "not a PE image"));
    }
    if !asset.sha256.is_empty() {
        let actual = sha256_hex(&bytes);
        if !actual.eq_ignore_ascii_case(&asset.sha256) {
            return Err(ApiError::new(
                "UPDATE_HASH",
                format!("{actual} != {}", asset.sha256),
            ));
        }
    }
    fs::write(dest, &bytes).map_err(|e| ApiError::new("UPDATE_IO", e.to_string()))
}

/// Moves the running image aside and copies the new one into its place. The old name is
/// kept so a failed copy can be rolled back; the next launch deletes it.
fn replace_portable(exe: &Path, downloaded: &Path) -> Result<(), ApiError> {
    let old = exe.with_extension("old.exe");
    let _ = fs::remove_file(&old);
    fs::rename(exe, &old).map_err(|e| ApiError::new("UPDATE_SWAP", e.to_string()))?;
    if let Err(e) = fs::copy(downloaded, exe) {
        let _ = fs::rename(&old, exe);
        return Err(ApiError::new("UPDATE_SWAP", e.to_string()));
    }
    Ok(())
}

/// Writes a small batch helper that polls for this PID and only then starts the new
/// executable (or runs the installer), avoiding a race with the single-instance plugin.
fn spawn_helper(body: &str) -> Result<(), ApiError> {
    let script = std::env::temp_dir().join(format!("flowdir-update-{}.cmd", std::process::id()));
    let text = format!(
        "@echo off\r\nsetlocal\r\n:wait\r\ntasklist /FI \"PID eq {}\" /NH | find /I \".exe\" >nul\r\nif not errorlevel 1 (\r\ntimeout /t 1 /nobreak >nul\r\ngoto wait\r\n)\r\n{}\r\ndel \"%~f0\"\r\n",
        std::process::id(),
        body
    );
    fs::write(&script, text).map_err(|e| ApiError::new("UPDATE_IO", e.to_string()))?;
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("cmd")
        .args(["/C", &script.to_string_lossy()])
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

        let policy: u32 = WINHTTP_OPTION_REDIRECT_POLICY_ALWAYS;
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

        let mut out = Vec::new();
        let mut buffer = vec![0u8; 64 * 1024];
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
        }
        Ok(out)
    }
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
    fn portable_swap_keeps_previous_image() {
        let dir = std::env::temp_dir().join(format!("flowdir-swap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("FlowDir.exe");
        let new = dir.join("new.exe");
        fs::write(&exe, b"old").unwrap();
        fs::write(&new, b"new").unwrap();
        replace_portable(&exe, &new).unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"new");
        assert_eq!(fs::read(dir.join("FlowDir.old.exe")).unwrap(), b"old");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn interval_gates_checks() {
        assert!(due(0, 24));
        assert!(due(now_secs() - 26 * 3600, 24));
        assert!(!due(now_secs() - 3600, 24));
        assert!(!due(0, 0));
    }
}
