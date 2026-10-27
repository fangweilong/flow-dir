//! Elevated retry for operations the shell refused with `ACCESS_DENIED`.
//!
//! A non-elevated GUI cannot raise its own token, so it re-launches this same executable
//! through `ShellExecuteExW(runas)` with `--elevated-op <request.json> --result <result.json>`.
//! `main` handles that branch before Tauri, the single-instance plugin or any window exists;
//! the helper runs the ordinary `IFileOperation` request and writes the outcome back.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

use crate::error::ApiError;
use crate::ops;

/// Long copies must not be cut off, but the GUI command thread cannot wait forever either.
const WAIT_TIMEOUT_MS: u32 = 10 * 60 * 1000;
/// `HRESULT_FROM_WIN32(ERROR_CANCELLED)`: the user dismissed the UAC prompt.
const ELEVATION_CANCELLED: u32 = 0x8007_04C7;

/// Parses the helper arguments from the process command line, skipping `argv[0]`.
pub fn helper_args() -> Option<(PathBuf, PathBuf)> {
    parse_helper_args(std::env::args_os().skip(1))
}

fn parse_helper_args<I: Iterator<Item = std::ffi::OsString>>(mut args: I) -> Option<(PathBuf, PathBuf)> {
    let mut request = None;
    let mut result = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--elevated-op") => request = args.next().map(PathBuf::from),
            Some("--result") => result = args.next().map(PathBuf::from),
            _ => {}
        }
    }
    Some((request?, result?))
}

/// Helper-process body: run the request synchronously, write the report and return an exit code.
pub fn run_helper(request_path: &Path, result_path: &Path) -> i32 {
    let report = std::fs::read(request_path)
        .map_err(|err| ApiError::new("BAD_ARGS", format!("elevated request: {err}")))
        .and_then(|bytes| {
            serde_json::from_slice::<ops::Request>(&bytes)
                .map_err(|err| ApiError::new("BAD_ARGS", format!("elevated request: {err}")))
        })
        .map(ops::run_headless)
        .unwrap_or_else(|err| ops::ElevatedReport {
            aborted: false,
            failed: Vec::new(),
            failed_total: 0,
            error: Some(err),
        });
    let _ = std::fs::remove_file(request_path);
    let Ok(bytes) = serde_json::to_vec(&report) else {
        return 2;
    };
    if std::fs::write(result_path, bytes).is_err() {
        return 2;
    }
    if report.error.is_none() && report.failed_total == 0 {
        0
    } else {
        1
    }
}

/// GUI side: elevates, waits for the helper and returns what it reported. Shell prompts
/// replace/overwrite UI through `IFileOperation`, exactly as in the non-elevated path.
pub fn request_elevated(
    app: &AppHandle,
    request: ops::Request,
) -> Result<ops::ElevatedReport, ApiError> {
    let dir = crate::store::cache_dir(app)?.join("elevated");
    std::fs::create_dir_all(&dir)
        .map_err(|err| ApiError::new("ELEVATED", format!("{}: {err}", dir.display())))?;
    prune(&dir);

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let request_path = dir.join(format!("{stamp}.request.json"));
    let result_path = dir.join(format!("{stamp}.result.json"));
    let json = serde_json::to_vec(&request)
        .map_err(|err| ApiError::new("ELEVATED", format!("request: {err}")))?;
    std::fs::write(&request_path, json)
        .map_err(|err| ApiError::new("ELEVATED", format!("{}: {err}", request_path.display())))?;

    let exe = std::env::current_exe()
        .map_err(|err| ApiError::new("ELEVATED", format!("current exe: {err}")))?;
    let params = params_for(&request_path, &result_path);

    let launch = {
        let exe = exe.clone();
        let result_path = result_path.clone();
        move || launch_and_wait(&exe, &params, &result_path)
    };
    let report = std::thread::Builder::new()
        .name("flowdir-elevate".into())
        .spawn(launch)
        .map_err(|err| ApiError::new("ELEVATED", err.to_string()))?
        .join()
        .map_err(|_| ApiError::new("ELEVATED", "elevation thread panicked"))?;
    let _ = std::fs::remove_file(&request_path);
    report
}

/// `--elevated-op "<request>" --result "<result>"` as a NUL-terminated UTF-16 command line.
fn params_for(request: &Path, result: &Path) -> Vec<u16> {
    let mut params: Vec<u16> = "--elevated-op \"".encode_utf16().collect();
    params.extend(request.as_os_str().encode_wide());
    params.extend("\" --result \"".encode_utf16());
    params.extend(result.as_os_str().encode_wide());
    params.push(u16::from(b'"'));
    params.push(0);
    params
}

fn launch_and_wait(
    exe: &Path,
    params: &[u16],
    result_path: &Path,
) -> Result<ops::ElevatedReport, ApiError> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let outcome = shell_execute(exe, params).and_then(|handle| {
            let wait = WaitForSingleObject(handle, WAIT_TIMEOUT_MS);
            let _ = CloseHandle(handle);
            if wait != WAIT_OBJECT_0 {
                return Err(ApiError::new(
                    "ELEVATED_TIMEOUT",
                    "the elevated operation is still running",
                ));
            }
            let bytes = std::fs::read(result_path).map_err(|err| {
                ApiError::new(
                    "ELEVATED_NO_RESULT",
                    format!("{}: {err}", result_path.display()),
                )
            })?;
            serde_json::from_slice(&bytes)
                .map_err(|err| ApiError::new("ELEVATED_NO_RESULT", err.to_string()))
        });
        CoUninitialize();
        outcome
    }
}

fn shell_execute(exe: &Path, params: &[u16]) -> Result<windows::Win32::Foundation::HANDLE, ApiError> {
    let exe_wide: Vec<u16> = exe.as_os_str().encode_wide().chain([0]).collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(exe_wide.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        if let Err(err) = ShellExecuteExW(&mut info) {
            let raw = err.code().0 as u32;
            if raw == ELEVATION_CANCELLED {
                return Err(ApiError::new("ELEVATION_CANCELED", "UAC prompt dismissed"));
            }
            return Err(ApiError::new(
                "ELEVATED_LAUNCH",
                format!("0x{raw:08X}: {err}"),
            ));
        }
    }
    if info.hProcess.is_invalid() {
        return Err(ApiError::new("ELEVATED_LAUNCH", "no helper process handle"));
    }
    Ok(info.hProcess)
}

/// Leaves no long-lived request/result files behind if a helper was killed by UAC or a reboot.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else { continue };
        if now
            .duration_since(modified)
            .map(|age| age.as_secs() > 86_400)
            .unwrap_or(false)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn helper_arguments_are_paired() {
        let args = [
            "--elevated-op",
            r"C:\Temp\req.json",
            "--result",
            r"C:\Temp\res.json",
        ];
        let (request, result) = parse_helper_args(args.into_iter().map(OsStr::new).map(|s| s.to_os_string()))
            .expect("both flags are present");
        assert_eq!(request, Path::new(r"C:\Temp\req.json"));
        assert_eq!(result, Path::new(r"C:\Temp\res.json"));
    }

    #[test]
    fn missing_helper_arguments_do_not_start_the_gui() {
        let args = ["--elevated-op", r"C:\Temp\req.json"];
        assert!(parse_helper_args(args.into_iter().map(OsStr::new).map(|s| s.to_os_string())).is_none());
    }
}
