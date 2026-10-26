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

    /// Shell operations report most per-item failures as copy-engine HRESULTs
    /// (`0x8027xxxx`, see `sherrors.h`), so they are mapped before the generic Win32 range.
    pub fn from_windows(path: &str, hresult: i32) -> Self {
        let raw = hresult as u32;
        let code = match raw {
            0x8027_0000 | 0x8027_0001 => "CANCELED",
            0x8027_0002 | 0x8027_0021 | 0x8027_0022 | 0x8027_003F => "ACCESS_DENIED",
            0x8027_0003 | 0x8027_0029..=0x8027_002C => "EXISTS",
            0x8027_0009 | 0x8027_000A | 0x8027_001B | 0x8027_001C | 0x8027_001F
            | 0x8027_0020 => "INVALID_TARGET",
            0x8027_000D => "FILE_TOO_LARGE",
            0x8027_000E | 0x8027_0032 | 0x8027_0033 => "DISK_FULL",
            0x8027_000F..=0x8027_0014 => "WRITE_PROTECT",
            0x8027_0015..=0x8027_001A => "DEVICE_NOT_READY",
            0x8027_001D | 0x8027_001E | 0x8027_003B | 0x8027_003C => "PATH_TOO_LONG",
            0x8027_0023 | 0x8027_0024 => "NOT_FOUND",
            0x8027_0025 | 0x8027_0026 | 0x8027_0035 => "DEVICE_NOT_READY",
            0x8027_0027 | 0x8027_0028 => "SHARE_UNAVAILABLE",
            0x8027_0037 | 0x8027_0038 | 0x8027_003A => "RECYCLE_UNAVAILABLE",
            0x8027_003D => "DIR_NOT_EMPTY",
            _ if raw & 0xFFFF_0000 == 0x8007_0000 => match raw & 0xFFFF {
                2 | 3 => "NOT_FOUND",
                5 => "ACCESS_DENIED",
                19 => "WRITE_PROTECT",
                21 => "DEVICE_NOT_READY",
                32 | 33 => "SHARE_UNAVAILABLE",
                39 | 112 => "DISK_FULL",
                80 | 183 => "EXISTS",
                123 => "INVALID_NAME",
                145 => "DIR_NOT_EMPTY",
                206 => "PATH_TOO_LONG",
                1223 => "CANCELED",
                _ => "WINDOWS",
            },
            _ => "WINDOWS",
        };
        Self::new(code, format!("{path}: 0x{raw:08X}"))
    }
}
