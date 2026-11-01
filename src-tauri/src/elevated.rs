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
use std::sync::Arc;
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
use windows::Win32::System::Threading::{
    CreateEventW, OpenEventW, SetEvent, TerminateProcess, WaitForMultipleObjects,
    WaitForSingleObject, INFINITE, SYNCHRONIZATION_SYNCHRONIZE,
};
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
pub fn helper_args() -> Option<(String, String, Option<String>)> {
    parse_helper_args(std::env::args_os().skip(1))
}

fn parse_helper_args<I: Iterator<Item = std::ffi::OsString>>(
    mut args: I,
) -> Option<(String, String, Option<String>)> {
    let mut pipe = None;
    let mut nonce = None;
    let mut cancel = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--elevated-op") => pipe = args.next().and_then(|v| v.to_str().map(str::to_string)),
            Some("--nonce") => nonce = args.next().and_then(|v| v.to_str().map(str::to_string)),
            Some("--cancel-event") => {
                cancel = args.next().and_then(|v| v.to_str().map(str::to_string))
            }
            _ => {}
        }
    }
    Some((pipe?, nonce?, cancel))
}

#[derive(Serialize, Deserialize)]
struct Payload {
    nonce: String,
    request: ops::Request,
}

/// Helper-process body: receives the request from the GUI pipe, runs it synchronously and
/// writes the report back over the same connection.
pub fn run_helper(pipe_name: &str, nonce: &str, cancel_event: Option<&str>) -> i32 {
    let report = helper_body(pipe_name, nonce, cancel_event);
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

fn helper_body(
    pipe_name: &str,
    nonce: &str,
    cancel_event: Option<&str>,
) -> Result<ops::ElevatedReport, ApiError> {
    let pipe = open_client(pipe_name)?;
    let payload: Payload = serde_json::from_slice(&read_message_sync(pipe.0)?)
        .map_err(|err| ApiError::new("BAD_ARGS", format!("elevated request: {err}")))?;
    if payload.nonce != nonce {
        return Err(ApiError::new("BAD_ARGS", "elevated nonce mismatch"));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(name) = cancel_event {
        watch_cancel(name, cancel.clone());
    }
    let report = ops::run_headless(payload.request, cancel);
    let bytes = serde_json::to_vec(&report)
        .map_err(|err| ApiError::new("BAD_ARGS", format!("elevated report: {err}")))?;
    write_message_sync(pipe.0, &bytes)?;
    Ok(report)
}

/// Watches the GUI's named event and flips the helper's cancel flag when it fires.
///
/// The medium-integrity GUI cannot always `TerminateProcess` a high-integrity helper, so
/// cancellation has to be cooperative: the helper is the only side that can stop the shell
/// operation, and it polls the same flag the in-process operations use.
fn watch_cancel(name: &str, cancel: Arc<AtomicBool>) {
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let Ok(handle) =
        (unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(wide.as_ptr())) })
    else {
        return;
    };
    let event = OwnedHandle(handle);
    let _ = std::thread::Builder::new()
        .name("flowdir-elevated-cancel".into())
        .spawn(move || {
            wait_event(&event);
            cancel.store(true, Ordering::Relaxed);
        });
}

/// Blocks until the event is signalled. Takes the wrapper whole so the closure moves the
/// `Send` handle type instead of the raw pointer inside it.
fn wait_event(event: &OwnedHandle) {
    unsafe {
        let _ = WaitForSingleObject(event.0, INFINITE);
    }
}

/// GUI side: elevates, sends the request over a fresh pipe and returns what the helper
/// reported. Cancel or timeout signals the helper's cancel event and only then falls back
/// to terminating it, because a medium-integrity process may be denied that on a
/// high-integrity child.
pub fn request_elevated(
    app: &AppHandle,
    request: ops::Request,
    cancel: std::sync::Arc<AtomicBool>,
) -> Result<ops::ElevatedReport, ApiError> {
    let _ = app;
    let nonce = random_hex();
    let pipe_name = format!(r"\\.\pipe\flowdir-elevated-{nonce}");
    let cancel_name = format!(r"flowdir-elevated-cancel-{nonce}");
    let cancel_event = create_event(&cancel_name)?;
    let exe = std::env::current_exe()
        .map_err(|err| ApiError::new("ELEVATED", format!("current exe: {err}")))?;
    let params = params_for(&pipe_name, &nonce, &cancel_name);
    std::thread::Builder::new()
        .name("flowdir-elevate".into())
        .spawn(move || {
            run_worker(
                &exe,
                &params,
                &pipe_name,
                &nonce,
                request,
                cancel,
                cancel_event,
            )
        })
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

/// `--elevated-op "<pipe>" --nonce "<nonce>" --cancel-event "<event>"` as a NUL-terminated
/// UTF-16 command line.
fn params_for(pipe_name: &str, nonce: &str, cancel_name: &str) -> Vec<u16> {
    let mut params: Vec<u16> = "--elevated-op \"".encode_utf16().collect();
    params.extend(pipe_name.encode_utf16());
    params.extend("\" --nonce \"".encode_utf16());
    params.extend(nonce.encode_utf16());
    params.extend("\" --cancel-event \"".encode_utf16());
    params.extend(cancel_name.encode_utf16());
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
    cancel_event: OwnedHandle,
) -> Result<ops::ElevatedReport, ApiError> {
    let pipe = create_server(pipe_name)?;
    let event = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
        .map_err(|err| ApiError::new("ELEVATED", err.to_string()))?;
    let event = OwnedHandle(event);

    let process = shell_execute(exe, params)?;
    let fork = OwnedHandle(process);
    let budget = Instant::now() + Duration::from_millis(u64::from(WAIT_TIMEOUT_MS));

    let result = (|| {
        // The helper may exit before it ever connects (credential-based elevation denies
        // the pipe to the other account). Waiting on the process as well turns a ten-minute
        // stall into an immediate, accurate error.
        let process = Some(fork.0);
        connect(pipe.0, event.0, process, &cancel, budget)?;
        let payload = serde_json::to_vec(&Payload {
            nonce: nonce.to_string(),
            request,
        })
        .map_err(|err| ApiError::new("ELEVATED", format!("request: {err}")))?;
        write_message(pipe.0, event.0, process, &payload, &cancel, budget)?;
        let bytes = read_message(pipe.0, event.0, process, &cancel, budget)?;
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
            // Cooperative stop first: `TerminateProcess` may be refused across integrity
            // levels, and the helper polls this event during the operation.
            let _ = SetEvent(cancel_event.0);
            let _ = WaitForSingleObject(fork.0, 10_000);
            let _ = TerminateProcess(fork.0, 1);
            let _ = WaitForSingleObject(fork.0, 5_000);
        },
    }
    result
}

/// Named event the GUI sets when it gives up; the helper opens the same name and turns it
/// into the cooperative cancel flag.
fn create_event(name: &str) -> Result<OwnedHandle, ApiError> {
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let handle = unsafe { CreateEventW(None, false, false, PCWSTR(wide.as_ptr())) }
        .map_err(|err| ApiError::new("ELEVATED", err.to_string()))?;
    Ok(OwnedHandle(handle))
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

fn connect(
    pipe: HANDLE,
    event: HANDLE,
    process: Option<HANDLE>,
    cancel: &AtomicBool,
    budget: Instant,
) -> Result<(), ApiError> {
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
                wait_io(pipe, &mut overlap, event, process, cancel, budget).map(|_| ())
            }
        }
    }
}

/// Cancels a pending overlapped operation and waits for the kernel to finish with
/// `overlap` and its buffer. `CancelIoEx` only marks the request: returning before the
/// driver completes it would free `OVERLAPPED` and the I/O buffer underneath a pending
/// write, so the completion is drained here even though the caller sees a cancellation.
unsafe fn cancel_and_drain(handle: HANDLE, overlap: &mut OVERLAPPED) {
    let _ = CancelIoEx(handle, Some(overlap));
    let mut bytes = 0u32;
    let _ = GetOverlappedResult(handle, overlap, &mut bytes, true);
}

/// Waits for an overlapped operation in short slices so cancellation and the overall
/// deadline stay responsive.
unsafe fn wait_io(
    handle: HANDLE,
    overlap: &mut OVERLAPPED,
    event: HANDLE,
    process: Option<HANDLE>,
    cancel: &AtomicBool,
    budget: Instant,
) -> Result<u32, ApiError> {
    loop {
        if cancel.load(Ordering::Relaxed) {
            cancel_and_drain(handle, overlap);
            return Err(ApiError::new("ELEVATION_CANCELED", "canceled"));
        }
        let ready = match process {
            Some(process) => unsafe { WaitForMultipleObjects(&[event, process], false, 200) },
            None => unsafe { WaitForSingleObject(event, 200) },
        };
        match ready {
            WAIT_OBJECT_0 => {
                let mut bytes = 0u32;
                GetOverlappedResult(handle, overlap, &mut bytes, false)
                    .map_err(|err| ApiError::new("ELEVATED_IO", err.to_string()))?;
                return Ok(bytes);
            }
            // Index 1 is the helper handle: it exited without answering.
            exited if exited.0 == WAIT_OBJECT_0.0 + 1 => {
                cancel_and_drain(handle, overlap);
                return Err(ApiError::new(
                    "ELEVATED_LAUNCH",
                    "the elevated helper exited before replying",
                ));
            }
            WAIT_TIMEOUT => {
                if Instant::now() >= budget {
                    cancel_and_drain(handle, overlap);
                    return Err(ApiError::new(
                        "ELEVATED_TIMEOUT",
                        "the elevated operation is still running",
                    ));
                }
            }
            _ => return Err(ApiError::new("ELEVATED_IO", "wait failed")),
        }
    }
}

fn write_message(
    pipe: HANDLE,
    event: HANDLE,
    process: Option<HANDLE>,
    bytes: &[u8],
    cancel: &AtomicBool,
    budget: Instant,
) -> Result<(), ApiError> {
    let len = u32::try_from(bytes.len())
        .map_err(|_| ApiError::new("ELEVATED", "request too large"))?;
    write_all(pipe, event, process, &len.to_le_bytes(), cancel, budget)?;
    write_all(pipe, event, process, bytes, cancel, budget)
}

fn write_all(
    pipe: HANDLE,
    event: HANDLE,
    process: Option<HANDLE>,
    bytes: &[u8],
    cancel: &AtomicBool,
    budget: Instant,
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
                    wait_io(pipe, &mut overlap, event, process, cancel, budget)?
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
    process: Option<HANDLE>,
    cancel: &AtomicBool,
    budget: Instant,
) -> Result<Vec<u8>, ApiError> {
    let mut header = [0u8; 4];
    read_exact(pipe, event, process, &mut header, cancel, budget)?;
    let len = u32::from_le_bytes(header);
    if len > MAX_MESSAGE {
        return Err(ApiError::new("ELEVATED", "response too large"));
    }
    let mut body = vec![0u8; len as usize];
    read_exact(pipe, event, process, &mut body, cancel, budget)?;
    Ok(body)
}

fn read_exact(
    pipe: HANDLE,
    event: HANDLE,
    process: Option<HANDLE>,
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
                    wait_io(pipe, &mut overlap, event, process, cancel, budget)?
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
            "--cancel-event",
            "flowdir-elevated-cancel-abc",
        ];
        let (pipe, nonce, cancel) =
            parse_helper_args(args.into_iter().map(OsStr::new).map(|s| s.to_os_string()))
                .expect("both flags are present");
        assert_eq!(pipe, r"\\.\pipe\flowdir-elevated-abc");
        assert_eq!(nonce, "abc");
        assert_eq!(cancel.as_deref(), Some("flowdir-elevated-cancel-abc"));
    }

    #[test]
    fn missing_helper_arguments_do_not_start_the_gui() {
        let args = ["--elevated-op", r"\\.\pipe\flowdir-elevated-abc"];
        assert!(parse_helper_args(args.into_iter().map(OsStr::new).map(|s| s.to_os_string())).is_none());
    }
}
