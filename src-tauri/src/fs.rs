use std::mem::offset_of;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    FileIdBothDirectoryInfo, GetDriveTypeW, GetFileInformationByHandleEx, GetLogicalDrives,
    FILE_ID_BOTH_DIR_INFO,
};

use crate::error::ApiError;

const ATTR_DIRECTORY: u32 = 0x10;
const ATTR_HIDDEN: u32 = 0x2;
const ATTR_SYSTEM: u32 = 0x4;
const FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

const HRESULT_NO_MORE_FILES: i32 = 0x8007_0012u32 as i32;
const HRESULT_MORE_DATA: i32 = 0x8007_00EAu32 as i32;

const WINDOWS_TICKS_PER_MS: i64 = 10_000;
const EPOCH_DIFF_MS: i64 = 11_644_473_600_000;
const MAX_ENTRIES: usize = 500_000;

#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub size: u64,
    pub mtime: u64,
    pub attrs: u32,
    pub id: u64,
    key: Vec<u8>,
    ext_key: Vec<u8>,
}

impl Entry {
    fn new(name: String, size: u64, mtime: u64, attrs: u32, id: u64) -> Self {
        let key = natural_key(&name);
        let ext_key = natural_key(
            name.rsplit_once('.')
                .map(|(_, ext)| ext)
                .unwrap_or("")
                .trim_end(),
        );
        Self {
            name,
            size,
            mtime,
            attrs,
            id,
            key,
            ext_key,
        }
    }

    pub fn is_dir(&self) -> bool {
        self.attrs & ATTR_DIRECTORY != 0
    }
}

/// Locale-agnostic sort key: lowercase, with digit runs zero-padded so that
/// "file2" sorts before "file10". CJK sorts by code point (locale collation
/// via LCMapStringEx is a later refinement).
fn natural_key(name: &str) -> Vec<u8> {
    let lower = name.to_lowercase();
    let bytes = lower.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() + 8);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let digits = lower[start..i].trim_start_matches('0');
            let digits = if digits.is_empty() { "0" } else { digits };
            for _ in 0..12usize.saturating_sub(digits.len()) {
                out.push(b'0');
            }
            out.extend_from_slice(digits.as_bytes());
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

pub fn enumerate(
    path: &str,
    show_hidden: bool,
    cancel: &AtomicBool,
) -> Result<Vec<Entry>, ApiError> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FLAG_BACKUP_SEMANTICS)
        .open(path)
        .map_err(|e| ApiError::from_io(path, &e))?;

    let handle = HANDLE(file.as_raw_handle());
    let mut buffer: Vec<u64> = vec![0; 8192];
    let mut entries: Vec<Entry> = Vec::new();

    'scan: loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let size = (buffer.len() * std::mem::size_of::<u64>()) as u32;
        let result = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileIdBothDirectoryInfo,
                buffer.as_mut_ptr().cast(),
                size,
            )
        };

        if let Err(err) = result {
            let code = err.code().0;
            if code == HRESULT_NO_MORE_FILES {
                break;
            }
            if code == HRESULT_MORE_DATA {
                buffer.resize(buffer.len() * 2, 0);
                continue;
            }
            return Err(ApiError::from_windows(path, code));
        }

        let base = buffer.as_ptr().cast::<u8>();
        let mut offset = 0usize;
        loop {
            let info = unsafe { &*(base.add(offset).cast::<FILE_ID_BOTH_DIR_INFO>()) };
            let name_len = info.FileNameLength as usize;
            let name_ptr =
                unsafe { base.add(offset + offset_of!(FILE_ID_BOTH_DIR_INFO, FileName)) }
                    .cast::<u16>();
            let name_slice = unsafe { std::slice::from_raw_parts(name_ptr, name_len / 2) };
            let is_dot = (name_slice.len() == 1 && name_slice[0] == 0x2E)
                || (name_slice.len() == 2 && name_slice[0] == 0x2E && name_slice[1] == 0x2E);

            if !is_dot && info.FileNameLength > 0 {
                let attrs = info.FileAttributes;
                let hidden = attrs & (ATTR_HIDDEN | ATTR_SYSTEM) != 0;
                if show_hidden || !hidden {
                    let name = std::ffi::OsString::from_wide(name_slice)
                        .to_string_lossy()
                        .into_owned();
                    let is_dir = attrs & ATTR_DIRECTORY != 0;
                    let size64 = if is_dir {
                        0
                    } else {
                        info.EndOfFile.max(0) as u64
                    };
                    let mtime =
                        (info.LastWriteTime / WINDOWS_TICKS_PER_MS - EPOCH_DIFF_MS).max(0) as u64;
                    entries.push(Entry::new(name, size64, mtime, attrs, info.FileId as u64));
                }
            }

            if info.NextEntryOffset == 0 {
                break;
            }
            offset += info.NextEntryOffset as usize;

            if entries.len() >= MAX_ENTRIES {
                break 'scan;
            }
        }
    }

    Ok(entries)
}

pub fn sort_entries(entries: &mut [Entry], key: &str, dir: &str) {
    let desc = dir == "desc";
    entries.sort_unstable_by(|a, b| {
        let folder_order = b.is_dir().cmp(&a.is_dir());
        if folder_order != std::cmp::Ordering::Equal {
            return folder_order;
        }
        let primary = match key {
            "size" => a.size.cmp(&b.size),
            "mtime" => a.mtime.cmp(&b.mtime),
            "type" => a.ext_key.cmp(&b.ext_key),
            _ => a.key.cmp(&b.key),
        };
        let primary = if desc { primary.reverse() } else { primary };
        primary
            .then_with(|| a.key.cmp(&b.key))
            .then_with(|| a.id.cmp(&b.id))
    });
}

#[derive(serde::Serialize, Clone)]
pub struct Drive {
    pub root: String,
    pub kind: String,
}

pub fn drives() -> Vec<Drive> {
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = char::from(b'A' + i as u8);
        let root = format!("{letter}:\\");
        let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
        let kind = match unsafe { GetDriveTypeW(PCWSTR(wide.as_ptr())) } {
            2 => "removable",
            3 => "fixed",
            4 => "network",
            5 => "cdrom",
            6 => "ramdisk",
            _ => continue,
        };
        out.push(Drive {
            root,
            kind: kind.to_string(),
        });
    }
    out
}

#[derive(serde::Serialize, Clone)]
pub struct Place {
    pub key: String,
    pub path: String,
}

pub fn quick_access() -> Vec<Place> {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    if home.is_empty() {
        return Vec::new();
    }
    let mut out = vec![Place {
        key: "home".to_string(),
        path: home.clone(),
    }];
    for (key, sub) in [
        ("desktop", "Desktop"),
        ("downloads", "Downloads"),
        ("documents", "Documents"),
    ] {
        let path = Path::new(&home).join(sub);
        if path.exists() {
            out.push(Place {
                key: key.to_string(),
                path: path.to_string_lossy().into_owned(),
            });
        }
    }
    out
}
