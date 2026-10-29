//! Elevated retry for operations the shell refused with `ACCESS_DENIED`.
//!
//! A non-elevated GUI cannot raise its own token, so it re-launches this same executable
//! through `ShellExecuteExW(runas)` with `--elevated-op <pipe> --nonce <nonce>`.
//! `main` handles that branch before Tauri, the single-instance plugin or any window exists.
//!
//! The request never touches the filesystem: the GUI hosts a one-instance named pipe whose
//! name contains a random nonce, the helper connects to it and receives `{nonce, request}`.
//! A same-user process can no longer swap a request file between the UAC prompt and the
//! helper start, which was the original TOCTOU.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED,
    GENERIC_READ, GENERIC_WRITE, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::Threading::{CreateEventW, TerminateProcess, WaitForSingleObject};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

use crate::error::ApiError;
use crate::ops;

/// Long copies must not be cut off, but the GUI command thread cannot wait forever either.
const WAIT_TIMEOUT_MS: u32 = 10 * 60 * 1000;
/// `HRESULT_FROM_WIN32(ERROR_CANCELLED)`: the user dismissed the UAC prompt.
const ELEVATION_CANCELLED: u32 = 0x8007_04C7;
const MAX_MESSAGE: u32 = 16 * 1024 * 1024;

struct OwnedHandle(HANDLE);

// The handle is only used by the elevation worker thread; it is moved into that thread.
unsafe impl Send for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Parses the helper arguments from the process command line, skipping `argv[0]`.
pub fn helper_args() -> Option<(String, String)> {
    parse_helper_args(std::env::args_os().skip(1))
}

fn parse_helper_args<I: Iterator<Item = std::ffi::OsString>>(
    mut args: I,
) -> Option<(String, String)> {
    let mut pipe = None;
    let mut nonce = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--elevated-op") => pipe = args.next().and_then(|v| v.to_str().map(str::to_string)),
            Some("--nonce") => nonce = args.next().and_then(|v| v.to_str().map(str::to_string)),
            _ => {}
        }
    }
    Some((pipe?, nonce?))
}

#[derive(Serialize, Deserialize)]
struct Payload {
    nonce: String,
    request: ops::Request,
}

/// Helper-process body: receives the request from the GUI pipe, runs it synchronously and
/// writes the report back over the same connection.
pub fn run_helper(pipe_name: &str, nonce: &str) -> i32 {
    let report = helper_body(pipe_name, nonce);
    match report {
        Ok(report) => {
            if report.error.is_none() && report.failed_total == 0 {
                0
            } else {
                1
            }
        }
        Err(_) => 2,
    }
}

fn helper_body(pipe_name: &str, nonce: &str) -> Result<ops::ElevatedReport, ApiError> {
    let pipe = open_client(pipe_name)?;
    let payload: Payload = serde_json::from_slice(&read_message_sync(pipe.0)?)
        .map_err(|err| ApiError::new("BAD_ARGS", format!("elevated request: {err}")))?;
    if payload.nonce != nonce {
        return Err(ApiError::new("BAD_ARGS", "elevated nonce mismatch"));
    }
    let report = ops::run_headless(payload.request);
    let bytes = serde_json::to_vec(&report)
        .map_err(|err| ApiError::new("BAD_ARGS", format!("elevated report: {err}")))?;
    write_message_sync(pipe.0, &bytes)?;
    Ok(report)
}

/// GUI side: elevates, sends the request over a fresh pipe and returns what the helper
/// reported. Cancel or timeout terminates the helper so it cannot keep copying in the
/// background after the UI gave up.
pub fn request_elevated(
    app: &AppHandle,
    request: ops::Request,
    cancel: std::sync::Arc<AtomicBool>,
) -> Result<ops::ElevatedReport, ApiError> {
    let _ = app;
    let nonce = random_hex();
    let pipe_name = format!(r"\\.\pipe\flowdir-elevated-{nonce}");
    let exe = std::env::current_exe()
        .map_err(|err| ApiError::new("ELEVATED", format!("current exe: {err}")))?;
    let params = params_for(&pipe_name, &nonce);
    std::thread::Builder::new()
        .name("flowdir-elevate".into())
        .spawn(move || run_worker(&exe, &params, &pipe_name, &nonce, request, cancel))
        .map_err(|err| ApiError::new("ELEVATED", err.to_string()))?
        .join()
        .map_err(|_| ApiError::new("ELEVATED", "elevation thread panicked"))?
}

fn random_hex() -> String {
    let mut bytes = [0u8; 16];
    unsafe {
        let _ = windows::Win32::Security::Cryptography::BCryptGenRandom(
            None,
            &mut bytes,
            windows::Win32::Security::Cryptography::BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        );
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `--elevated-op "<pipe>" --nonce "<nonce>"` as a NUL-terminated UTF-16 command line.
fn params_for(pipe_name: &str, nonce: &str) -> Vec<u16> {
    let mut params: Vec<u16> = "--elevated-op \"".encode_utf16().collect();
    params.extend(pipe_name.encode_utf16());
    params.extend("\" --nonce \"".encode_utf16());
    params.extend(nonce.encode_utf16());
    params.push(u16::from(b'"'));
    params.push(0);
    params
}

fn run_worker(
    exe: &Path,
    params: &[u16],
    pipe_name: &str,
    nonce: &str,
    request: ops::Request,
    cancel: std::sync::Arc<AtomicBool>,
) -> Result<ops::ElevatedReport, ApiError> {
    let pipe = create_server(pipe_name)?;
    let event = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
        .map_err(|err| ApiError::new("ELEVATED", err.to_string()))?;
    let event = OwnedHandle(event);

    let process = shell_execute(exe, params)?;
    let fork = OwnedHandle(process);
    let budget = Instant::now() + Duration::from_millis(u64::from(WAIT_TIMEOUT_MS));

    let result = (|| {
        connect(pipe.0, event.0, &cancel)?;
        let payload = serde_json::to_vec(&Payload {
            nonce: nonce.to_string(),
            request,
        })
        .map_err(|err| ApiError::new("ELEVATED", format!("request: {err}")))?;
        write_message(pipe.0, event.0, &payload, &cancel)?;
        let bytes = read_message(pipe.0, event.0, &cancel, budget)?;
        serde_json::from_slice(&bytes)
            .map_err(|err| ApiError::new("ELEVATED_NO_RESULT", err.to_string()))
    })();

    match &result {
        Ok(_) => {
            // The helper writes the report just before exiting; give it a moment.
            unsafe {
                let _ = WaitForSingleObject(fork.0, 10_000);
            }
        }
        Err(_) => unsafe {
            let _ = TerminateProcess(fork.0, 1);
            let _ = WaitForSingleObject(fork.0, 5_000);
        },
    }
    result
}

fn create_server(name: &str) -> Result<OwnedHandle, ApiError> {
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(wide.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            64 * 1024,
            64 * 1024,
            0,
            None,
        )
    };
    if handle.is_invalid() {
        return Err(ApiError::new("ELEVATED", "CreateNamedPipe failed"));
    }
    Ok(OwnedHandle(handle))
}

fn open_client(name: &str) -> Result<OwnedHandle, ApiError> {
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .map_err(|err| ApiError::new("ELEVATED", format!("connect pipe: {err}")))?;
    Ok(OwnedHandle(handle))
}

fn connect(pipe: HANDLE, event: HANDLE, cancel: &AtomicBool) -> Result<(), ApiError> {
    unsafe {
        let mut overlap = OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };
        match ConnectNamedPipe(pipe, Some(&mut overlap)) {
            Ok(()) => Ok(()),
            Err(_) => {
                let code = GetLastError();
                if code == ERROR_PIPE_CONNECTED {
                    return Ok(());
                }
                if code != ERROR_IO_PENDING {
                    return Err(ApiError::new(
                        "ELEVATED",
                        format!("ConnectNamedPipe: {}", code.0),
                    ));
                }
                wait_io(pipe, &mut overlap, event, cancel).map(|_| ())
            }
        }
    }
}

/// Waits for an overlapped operation in short slices so cancellation stays responsive.
unsafe fn wait_io(
    handle: HANDLE,
    overlap: &mut OVERLAPPED,
    event: HANDLE,
    cancel: &AtomicBool,
) -> Result<u32, ApiError> {
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = CancelIoEx(handle, Some(overlap));
            return Err(ApiError::new("ELEVATION_CANCELED", "canceled"));
        }
        match WaitForSingleObject(event, 200) {
            WAIT_OBJECT_0 => {
                let mut bytes = 0u32;
                GetOverlappedResult(handle, overlap, &mut bytes, false)
                    .map_err(|err| ApiError::new("ELEVATED_IO", err.to_string()))?;
                return Ok(bytes);
            }
            WAIT_TIMEOUT => {}
            _ => return Err(ApiError::new("ELEVATED_IO", "wait failed")),
        }
    }
}

fn write_message(
    pipe: HANDLE,
    event: HANDLE,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<(), ApiError> {
    let len = u32::try_from(bytes.len())
        .map_err(|_| ApiError::new("ELEVATED", "request too large"))?;
    write_all(pipe, event, &len.to_le_bytes(), cancel)?;
    write_all(pipe, event, bytes, cancel)
}

fn write_all(
    pipe: HANDLE,
    event: HANDLE,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<(), ApiError> {
    let mut offset = 0;
    while offset < bytes.len() {
        unsafe {
            let mut overlap = OVERLAPPED {
                hEvent: event,
                ..Default::default()
            };
            let written = match WriteFile(pipe, Some(&bytes[offset..]), None, Some(&mut overlap)) {
                Ok(()) => {
                    let mut count = 0u32;
                    GetOverlappedResult(pipe, &overlap, &mut count, true)
                        .map_err(|err| ApiError::new("ELEVATED_IO", err.to_string()))?;
                    count
                }
                Err(_) if GetLastError() == ERROR_IO_PENDING => {
                    wait_io(pipe, &mut overlap, event, cancel)?
                }
                Err(_) => {
                    let code = GetLastError();
                    return Err(ApiError::new(
                        "ELEVATED_IO",
                        format!("WriteFile: {}", code.0),
                    ));
                }
            };
            if written == 0 {
                return Err(ApiError::new("ELEVATED_IO", "pipe closed while writing"));
            }
            offset += written as usize;
        }
    }
    Ok(())
}

fn read_message(
    pipe: HANDLE,
    event: HANDLE,
    cancel: &AtomicBool,
    budget: Instant,
) -> Result<Vec<u8>, ApiError> {
    let mut header = [0u8; 4];
    read_exact(pipe, event, &mut header, cancel, budget)?;
    let len = u32::from_le_bytes(header);
    if len > MAX_MESSAGE {
        return Err(ApiError::new("ELEVATED", "response too large"));
    }
    let mut body = vec![0u8; len as usize];
    read_exact(pipe, event, &mut body, cancel, budget)?;
    Ok(body)
}

fn read_exact(
    pipe: HANDLE,
    event: HANDLE,
    out: &mut [u8],
    cancel: &AtomicBool,
    budget: Instant,
) -> Result<(), ApiError> {
    let mut offset = 0;
    while offset < out.len() {
        if Instant::now() >= budget {
            return Err(ApiError::new(
                "ELEVATED_TIMEOUT",
                "the elevated operation is still running",
            ));
        }
        unsafe {
            let mut overlap = OVERLAPPED {
                hEvent: event,
                ..Default::default()
            };
            let read = match ReadFile(pipe, Some(&mut out[offset..]), None, Some(&mut overlap)) {
                Ok(()) => {
                    let mut count = 0u32;
                    GetOverlappedResult(pipe, &overlap, &mut count, true)
                        .map_err(|err| ApiError::new("ELEVATED_IO", err.to_string()))?;
                    count
                }
                Err(_) if GetLastError() == ERROR_IO_PENDING => {
                    wait_io(pipe, &mut overlap, event, cancel)?
                }
                Err(_) => {
                    let code = GetLastError();
                    if code == ERROR_BROKEN_PIPE {
                        return Err(ApiError::new(
                            "ELEVATED_NO_RESULT",
                            "helper closed the pipe without a report",
                        ));
                    }
                    return Err(ApiError::new("ELEVATED_IO", format!("ReadFile: {}", code.0)));
                }
            };
            if read == 0 {
                return Err(ApiError::new(
                    "ELEVATED_NO_RESULT",
                    "helper closed the pipe without a report",
                ));
            }
            offset += read as usize;
        }
    }
    Ok(())
}

fn read_message_sync(pipe: HANDLE) -> Result<Vec<u8>, ApiError> {
    let mut header = [0u8; 4];
    read_exact_sync(pipe, &mut header)?;
    let len = u32::from_le_bytes(header);
    if len > MAX_MESSAGE {
        return Err(ApiError::new("ELEVATED", "request too large"));
    }
    let mut body = vec![0u8; len as usize];
    read_exact_sync(pipe, &mut body)?;
    Ok(body)
}

fn read_exact_sync(pipe: HANDLE, out: &mut [u8]) -> Result<(), ApiError> {
    let mut offset = 0;
    while offset < out.len() {
        let mut read = 0u32;
        unsafe {
            ReadFile(pipe, Some(&mut out[offset..]), Some(&mut read), None)
                .map_err(|err| ApiError::new("ELEVATED_IO", err.to_string()))?;
        }
        if read == 0 {
            return Err(ApiError::new("ELEVATED_IO", "pipe closed"));
        }
        offset += read as usize;
    }
    Ok(())
}

fn write_message_sync(pipe: HANDLE, bytes: &[u8]) -> Result<(), ApiError> {
    let len = u32::try_from(bytes.len())
        .map_err(|_| ApiError::new("ELEVATED", "report too large"))?;
    write_all_sync(pipe, &len.to_le_bytes())?;
    write_all_sync(pipe, bytes)
}

fn write_all_sync(pipe: HANDLE, bytes: &[u8]) -> Result<(), ApiError> {
    let mut offset = 0;
    while offset < bytes.len() {
        let mut written = 0u32;
        unsafe {
            WriteFile(pipe, Some(&bytes[offset..]), Some(&mut written), None)
                .map_err(|err| ApiError::new("ELEVATED_IO", err.to_string()))?;
        }
        if written == 0 {
            return Err(ApiError::new("ELEVATED_IO", "pipe closed while writing"));
        }
        offset += written as usize;
    }
    Ok(())
}

fn shell_execute(exe: &Path, params: &[u16]) -> Result<HANDLE, ApiError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn helper_arguments_are_paired() {
        let args = [
            "--elevated-op",
            r"\\.\pipe\flowdir-elevated-abc",
            "--nonce",
            "abc",
        ];
        let (pipe, nonce) =
            parse_helper_args(args.into_iter().map(OsStr::new).map(|s| s.to_os_string()))
                .expect("both flags are present");
        assert_eq!(pipe, r"\\.\pipe\flowdir-elevated-abc");
        assert_eq!(nonce, "abc");
    }

    #[test]
    fn missing_helper_arguments_do_not_start_the_gui() {
        let args = ["--elevated-op", r"\\.\pipe\flowdir-elevated-abc"];
        assert!(parse_helper_args(args.into_iter().map(OsStr::new).map(|s| s.to_os_string())).is_none());
    }
}
