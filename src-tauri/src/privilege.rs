//! Process integrity helpers: report elevation and restart the GUI at the other level.
//!
//! The GUI is meant to run with normal rights (file operations that need more go through
//! the elevated helper), but an elevated parent — the installer's finish page, a terminal
//! started as administrator, or an update inherited from an elevated instance — leaves it
//! elevated. Windows UIPI then blocks drag-out into normal apps, so the title bar offers a
//! restart switch instead of leaving the user with a silent failure.

use std::mem::size_of;
use std::path::Path;
use std::time::Duration;

use tauri::{AppHandle, Runtime};
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    DuplicateTokenEx, GetTokenInformation, SecurityImpersonation, TokenElevation, TokenPrimary,
    TOKEN_ALL_ACCESS, TOKEN_DUPLICATE, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
    CreateProcessWithTokenW, GetCurrentProcess, OpenProcess, OpenProcessToken,
    CREATE_UNICODE_ENVIRONMENT, LOGON_WITH_PROFILE, PROCESS_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::Shell::{
    ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetShellWindow, GetWindowThreadProcessId, SW_SHOWNORMAL,
};

use crate::error::ApiError;

/// Whether this process runs with an elevated token.
pub fn is_elevated() -> bool {
    let mut token = HANDLE::default();
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut core::ffi::c_void),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

/// Whether Windows' User Account Control is enabled. With it disabled, an administrator
/// account gets an elevated token for every process (including Explorer), so there is no
/// lower level to switch to — the title bar explains that instead of offering a no-op.
pub fn uac_enabled() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};

    let subkey: Vec<u16> = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let value: Vec<u16> = "EnableLUA".encode_utf16().chain(std::iter::once(0)).collect();
    let mut enabled = 1u32;
    let mut size = 4u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut enabled as *mut _ as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    // A missing value means the default (enabled) on supported Windows versions.
    status.is_err() || enabled != 0
}

/// Starts a copy of this executable at the requested level and then shuts this process
/// down. The new instance waits for our PID before Tauri starts (see
/// [`restart_wait_pid`]), so the single-instance plugin cannot forward the relaunch back
/// into the instance that is on its way out.
pub fn restart<R: Runtime>(app: &AppHandle<R>, elevated: bool) -> Result<(), ApiError> {
    if is_elevated() == elevated {
        return Err(ApiError::new("PRIV_ALREADY", "already running at the requested level"));
    }
    let exe = std::env::current_exe().map_err(|e| ApiError::new("PRIVILEGE", e.to_string()))?;
    let args = format!("--restart-wait={}", std::process::id());
    if elevated {
        launch_elevated(&exe, &args)?;
    } else {
        launch_as_shell(&exe, &args)?;
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        // Let the IPC reply land (and the elevated copy get past UAC) before the window
        // disappears; the new instance waits for this PID anyway.
        std::thread::sleep(Duration::from_millis(250));
        crate::shutdown(&handle, false);
    });
    Ok(())
}

/// PID of the instance this process must let exit before starting its own Tauri runtime.
pub fn restart_wait_pid() -> Option<u32> {
    // `args()` panics on an argument that is not valid Unicode, and this runs before the
    // panic hook is installed. Only the ASCII switch matters here, so non-Unicode
    // arguments are skipped.
    std::env::args_os()
        .skip(1)
        .filter_map(|arg| arg.into_string().ok())
        .find_map(|arg| arg.strip_prefix("--restart-wait=")?.parse().ok())
}

/// Waits for the previous instance to release the executable and the single-instance
/// mutex. A missing process (already exited) returns immediately.
pub fn wait_for_exit(pid: u32) {
    use windows::Win32::System::Threading::{WaitForSingleObject, PROCESS_SYNCHRONIZE};
    unsafe {
        if let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(handle, 20_000);
            let _ = CloseHandle(handle);
        }
    }
}

/// Elevation always goes through the shell's `runas` verb so Windows shows the UAC
/// prompt; the caller stays at its current level when the user declines.
fn launch_elevated(exe: &Path, args: &str) -> Result<(), ApiError> {
    let file = windows::core::HSTRING::from(exe);
    let params = windows::core::HSTRING::from(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.map_err(|err| {
        // ERROR_CANCELLED: the user declined the elevation prompt; that is not a bug.
        if err.code().0 as u32 == 0x8007_04C7 {
            ApiError::new("ELEVATION_CANCELED", "user declined the elevation prompt")
        } else {
            ApiError::new("PRIVILEGE", err.to_string())
        }
    })?;
    if !info.hProcess.is_invalid() {
        let _ = unsafe { CloseHandle(info.hProcess) };
    }
    Ok(())
}

/// De-elevates by launching through the interactive shell: duplicating Explorer's token
/// gives the new process the desktop user's normal integrity level. `ShellExecute` from
/// an elevated process would simply inherit the elevated token.
fn launch_as_shell(exe: &Path, args: &str) -> Result<(), ApiError> {
    unsafe {
        let shell = GetShellWindow();
        if shell.is_invalid() {
            return Err(ApiError::new("PRIVILEGE", "shell window not found"));
        }
        let mut shell_pid = 0u32;
        GetWindowThreadProcessId(shell, Some(&mut shell_pid));
        if shell_pid == 0 {
            return Err(ApiError::new("PRIVILEGE", "shell process not found"));
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, shell_pid)
            .map_err(|e| ApiError::new("PRIVILEGE", e.to_string()))?;
        let mut token = HANDLE::default();
        if let Err(err) = OpenProcessToken(process, TOKEN_DUPLICATE | TOKEN_QUERY, &mut token) {
            let _ = CloseHandle(process);
            return Err(ApiError::new("PRIVILEGE", err.to_string()));
        }
        let mut primary = HANDLE::default();
        let duplicated = DuplicateTokenEx(
            token,
            TOKEN_ALL_ACCESS,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut primary,
        );
        let _ = CloseHandle(process);
        let _ = CloseHandle(token);
        duplicated.map_err(|e| ApiError::new("PRIVILEGE", e.to_string()))?;

        let mut command: Vec<u16> = format!("\"{}\" {args}", exe.display())
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let startup = STARTUPINFOW {
            cb: size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut child = PROCESS_INFORMATION::default();
        let spawned = CreateProcessWithTokenW(
            primary,
            LOGON_WITH_PROFILE,
            PCWSTR::null(),
            Some(PWSTR(command.as_mut_ptr())),
            CREATE_UNICODE_ENVIRONMENT,
            None,
            PCWSTR::null(),
            &startup,
            &mut child,
        );
        let _ = CloseHandle(primary);
        spawned.map_err(|e| ApiError::new("PRIVILEGE", e.to_string()))?;
        let _ = CloseHandle(child.hProcess);
        let _ = CloseHandle(child.hThread);
        Ok(())
    }
}
