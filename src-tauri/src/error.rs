use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct ApiError {
    pub code: String,
    pub detail: String,
}

impl ApiError {
    pub fn new(code: &str, detail: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            detail: detail.into(),
        }
    }

    pub fn from_io(path: &str, err: &std::io::Error) -> Self {
        use std::io::ErrorKind;
        let code = match err.kind() {
            ErrorKind::NotFound => "NOT_FOUND",
            ErrorKind::PermissionDenied => "ACCESS_DENIED",
            _ => "IO",
        };
        Self::new(code, format!("{path}: {err}"))
    }

    pub fn from_windows(path: &str, hresult: i32) -> Self {
        let code = match (hresult as u32) & 0xFFFF {
            2 | 3 => "NOT_FOUND",
            5 => "ACCESS_DENIED",
            32 | 33 => "SHARE_UNAVAILABLE",
            123 => "INVALID_NAME",
            206 => "PATH_TOO_LONG",
            _ => "WINDOWS",
        };
        Self::new(code, format!("{path}: 0x{:08X}", hresult as u32))
    }
}
