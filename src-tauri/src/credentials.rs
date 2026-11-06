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
pub fn save(id: &str, username: &str, password: &str) -> Result<(), ApiError> {
    if password.is_empty() {
        return delete(id);
    }
    let mut target_name = target(id);
    let mut user_name = wide(username);
    let mut blob = password.as_bytes().to_vec();
    let credential = CREDENTIALW {
        Flags: Default::default(),
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target_name.as_mut_ptr()),
        Comment: PWSTR::null(),
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

/// Stored password for `id`, or `None` when the user never saved one.
pub fn load(id: &str) -> Option<String> {
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
    let text = unsafe {
        let raw = &*credential;
        // The blob is not NUL-terminated and its length is authoritative; passwords are
        // stored as UTF-8 bytes.
        let bytes = if raw.CredentialBlob.is_null() || raw.CredentialBlobSize == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(raw.CredentialBlob, raw.CredentialBlobSize as usize).to_vec()
        };
        CredFree(credential as *const core::ffi::c_void);
        String::from_utf8(bytes).ok()
    };
    text.filter(|value| !value.is_empty())
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
