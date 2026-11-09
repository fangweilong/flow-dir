//! WebDAV passwords in Windows Credential Manager.
//!
//! Passwords never enter `settings.toml` or any IPC response: the frontend can set,
//! clear and query whether one exists, but only this module reads the blob, and it hands
//! it directly to WinHTTP. The storage is per Windows user (`CRED_PERSIST_LOCAL_MACHINE`)
//! and encrypted by the OS.

use std::ptr;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};

use crate::error::ApiError;

/// Serializes the read-modify-write in [`bind`] against a concurrent [`save`]; without it
/// the bind can write the old password back over a password the user just changed.
static CREDENTIAL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One target per connection id. Changing the prefix orphans every stored password, so
/// keep it stable across releases.
fn target(id: &str) -> Vec<u16> {
    format!("FlowDir/WebDAV/{id}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Stores `password` for `id`; an empty password removes the entry instead of saving a
/// blank one, which is what the settings page's "clear" action wants.
///
/// `origin` (scheme, host and port) is kept in the credential's comment. It is not a
/// secret: its purpose is to refuse the stored password when the connection's URL is later
/// edited to point at another server, so credentials are never sent to a new host.
pub fn save(
    id: &str,
    username: &str,
    password: &str,
    origin: Option<&str>,
) -> Result<(), ApiError> {
    let _guard = CREDENTIAL_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    save_locked(id, username, password, origin)
}

fn save_locked(
    id: &str,
    username: &str,
    password: &str,
    origin: Option<&str>,
) -> Result<(), ApiError> {
    if password.is_empty() {
        return delete(id);
    }
    let mut target_name = target(id);
    let mut user_name = wide(username);
    let mut comment = origin.map(wide);
    let mut blob = password.as_bytes().to_vec();
    let credential = CREDENTIALW {
        Flags: Default::default(),
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target_name.as_mut_ptr()),
        Comment: comment
            .as_mut()
            .map(|text| PWSTR(text.as_mut_ptr()))
            .unwrap_or(PWSTR::null()),
        LastWritten: Default::default(),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        AttributeCount: 0,
        Attributes: ptr::null_mut(),
        TargetAlias: PWSTR::null(),
        UserName: PWSTR(user_name.as_mut_ptr()),
    };
    unsafe { CredWriteW(&credential, 0) }
        .map_err(|e| ApiError::new("WEBDAV_CREDENTIAL", e.to_string()))
}

/// One stored credential: the secret plus the non-secret metadata needed to keep it bound
/// to the server it was entered for.
pub struct Stored {
    pub password: String,
    pub origin: Option<String>,
    pub username: Option<String>,
}

/// Stored password and the origin it is bound to, or `None` when the user never saved one.
pub fn load_with_origin(id: &str) -> Option<Stored> {
    let name = target(id);
    let mut credential: *mut CREDENTIALW = ptr::null_mut();
    // "Not found" arrives as an error, which is the same answer as an absent entry.
    unsafe {
        CredReadW(
            PCWSTR(name.as_ptr()),
            CRED_TYPE_GENERIC,
            None,
            &mut credential,
        )
    }
    .ok()?;
    if credential.is_null() {
        return None;
    }
    let stored = unsafe {
        let raw = &*credential;
        // The blob is not NUL-terminated and its length is authoritative; passwords are
        // stored as UTF-8 bytes.
        let bytes = if raw.CredentialBlob.is_null() || raw.CredentialBlobSize == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(raw.CredentialBlob, raw.CredentialBlobSize as usize).to_vec()
        };
        // `Comment` and `UserName` are NUL-terminated strings owned by the credential;
        // copy them before `CredFree` releases the block.
        let text = |pointer: PWSTR| {
            if pointer.is_null() {
                None
            } else {
                pointer.to_string().ok().filter(|value| !value.is_empty())
            }
        };
        let origin = text(raw.Comment);
        let username = text(raw.UserName);
        let password = String::from_utf8(bytes).ok();
        CredFree(credential as *const core::ffi::c_void);
        Stored {
            password: password?,
            origin,
            username,
        }
    };
    (!stored.password.is_empty()).then_some(stored)
}

/// Stored password for `id`, or `None` when the user never saved one.
pub fn load(id: &str) -> Option<String> {
    load_with_origin(id).map(|stored| stored.password)
}

/// Binds an existing credential to `origin` without changing the password. Used once for
/// entries written before the origin was recorded.
pub fn bind(id: &str, origin: &str) -> Result<(), ApiError> {
    let _guard = CREDENTIAL_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let Some(stored) = load_with_origin(id) else {
        return Ok(());
    };
    save_locked(
        id,
        stored.username.as_deref().unwrap_or(""),
        &stored.password,
        Some(origin),
    )
}

/// True when a password is stored, without exposing its value to the caller.
pub fn has(id: &str) -> bool {
    load(id).is_some()
}

/// Removes the stored password. A missing entry is not an error, so deleting a connection
/// twice (or one that never had a password) stays idempotent.
pub fn delete(id: &str) -> Result<(), ApiError> {
    let name = target(id);
    let result = unsafe { CredDeleteW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None) };
    match result {
        Ok(()) => Ok(()),
        Err(err) if err.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
        Err(err) => Err(ApiError::new("WEBDAV_CREDENTIAL", err.to_string())),
    }
}
