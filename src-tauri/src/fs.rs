use std::ffi::c_void;
use std::mem::offset_of;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    FileIdBothDirectoryInfo, GetDriveTypeW, GetFileInformationByHandleEx, GetLogicalDrives,
    FILE_ID_BOTH_DIR_INFO,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, IPersistFile, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::UI::Shell::{
    IShellLinkW, SLGP_RAWPATH, ShellLink, StrCmpLogicalW, FOLDERID_Desktop, FOLDERID_Documents,
    FOLDERID_Downloads, FOLDERID_Profile,
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
    ext_key: Vec<u8>,
    /// NUL-terminated UTF-16 of `name`, kept so the Explorer collation can compare
    /// without allocating once per comparison.
    wide: Vec<u16>,
}

impl Entry {
    fn new(name: String, size: u64, mtime: u64, attrs: u32, id: u64) -> Self {
        let ext_key = natural_key(
            name.rsplit_once('.')
                .map(|(_, ext)| ext)
                .unwrap_or("")
                .trim_end(),
        );
        let wide = name.encode_utf16().chain(std::iter::once(0)).collect();
        Self {
            name,
            size,
            mtime,
            attrs,
            id,
            ext_key,
            wide,
        }
    }

    pub fn is_dir(&self) -> bool {
        self.attrs & ATTR_DIRECTORY != 0
    }
}

/// Locale-agnostic sort key: lowercase, with digit runs zero-padded so that
/// "file2" sorts before "file10". Used for extension/type ordering, where Explorer has
/// no linguistic rules; file names themselves go through {@link logical_cmp}.
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
            out.resize(out.len() + 12usize.saturating_sub(digits.len()), b'0');
            out.extend_from_slice(digits.as_bytes());
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

/// One directory read: the rows plus whether `limit` cut the enumeration short.
pub struct ScanOutcome {
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

/// Reads a folder, reporting how many rows have been read so a slow volume can show
/// progress before the sort, and whether the entry cap was reached.
pub fn enumerate_reporting(
    path: &str,
    show_hidden: bool,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize),
) -> Result<ScanOutcome, ApiError> {
    scan(path, show_hidden, false, MAX_ENTRIES, cancel, progress)
}

/// Subdirectories only, for the sidebar tree: files are dropped before their
/// metadata is touched, so folders with tens of thousands of files stay cheap.
pub fn enumerate_dirs(
    path: &str,
    show_hidden: bool,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<ScanOutcome, ApiError> {
    scan(path, show_hidden, true, limit, cancel, &mut |_| {})
}

fn scan(
    path: &str,
    show_hidden: bool,
    dirs_only: bool,
    limit: usize,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize),
) -> Result<ScanOutcome, ApiError> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FLAG_BACKUP_SEMANTICS)
        .open(path)
        .map_err(|e| ApiError::from_io(path, &e))?;

    let handle = HANDLE(file.as_raw_handle());
    let mut buffer: Vec<u64> = vec![0; 8192];
    let mut entries: Vec<Entry> = Vec::new();
    let mut truncated = false;

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
            if unsupported_directory_info(code) {
                // Some SMB/NAS servers reject FileIdBothDirectoryInfo outright. Restart
                // through read_dir instead of failing the whole folder.
                log::warn!(
                    target: "flowdir::fs",
                    "FileIdBothDirectoryInfo unsupported for {path} ({code:#010x}); using read_dir"
                );
                return scan_fallback(path, show_hidden, dirs_only, limit, cancel, progress);
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
                let is_dir = attrs & ATTR_DIRECTORY != 0;
                // Keep the offset advance below reachable: a `continue` here would
                // re-read the same record forever.
                if (show_hidden || !hidden) && (is_dir || !dirs_only) {
                    // One extra matching record proves `truncated`; a folder holding
                    // exactly `limit` entries must not claim it was cut short.
                    if entries.len() >= limit {
                        truncated = true;
                        break 'scan;
                    }
                    let name = std::ffi::OsString::from_wide(name_slice)
                        .to_string_lossy()
                        .into_owned();
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
        }
        // One call per buffer chunk: enough for a progress readout without flooding IPC.
        progress(entries.len());
    }

    Ok(ScanOutcome { entries, truncated })
}

/// `ERROR_INVALID_FUNCTION`, `ERROR_NOT_SUPPORTED`, `ERROR_INVALID_PARAMETER` and
/// `ERROR_INVALID_LEVEL`, i.e. the server does not implement this information class.
fn unsupported_directory_info(code: i32) -> bool {
    matches!(code as u32, 0x8007_0001 | 0x8007_0032 | 0x8007_0057 | 0x8007_007C)
}

/// Portable fallback for shares that reject `FileIdBothDirectoryInfo`. It is slower
/// (one metadata query per entry) but preserves the same filtering, limit and progress
/// semantics; entry ids are unavailable, which only affects tie-breaking in the sort.
fn scan_fallback(
    path: &str,
    show_hidden: bool,
    dirs_only: bool,
    limit: usize,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize),
) -> Result<ScanOutcome, ApiError> {
    use std::os::windows::fs::MetadataExt;

    let dir = std::fs::read_dir(path).map_err(|e| ApiError::from_io(path, &e))?;
    let mut entries: Vec<Entry> = Vec::new();
    let mut truncated = false;
    let mut seen = 0usize;
    for entry in dir {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(entry) = entry else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        let attrs = meta.file_attributes();
        let is_dir = attrs & ATTR_DIRECTORY != 0;
        if dirs_only && !is_dir {
            continue;
        }
        if !show_hidden && attrs & (ATTR_HIDDEN | ATTR_SYSTEM) != 0 {
            continue;
        }
        let size = if is_dir { 0 } else { meta.len() };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        let name = entry.file_name().to_string_lossy().into_owned();
        entries.push(Entry::new(name, size, mtime, attrs, 0));
        seen += 1;
        if seen.is_multiple_of(200) {
            progress(entries.len());
        }
        // One extra match proves there is more to read; truncating here keeps the
        // `truncated` flag accurate for folders that hold exactly `limit` entries.
        if entries.len() > limit {
            entries.truncate(limit);
            truncated = true;
            break;
        }
    }
    progress(entries.len());
    Ok(ScanOutcome { entries, truncated })
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
            _ => logical_wide(&a.wide, &b.wide),
        };
        let primary = if desc { primary.reverse() } else { primary };
        primary
            .then_with(|| logical_wide(&a.wide, &b.wide))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Explorer's own natural-order comparison (`StrCmpLogicalW`): case-insensitive,
/// digit runs compare numerically, and non-ASCII follows the user's locale, which is
/// what makes a Chinese file list match the one File Explorer shows.
fn logical_wide(a: &[u16], b: &[u16]) -> std::cmp::Ordering {
    let result = unsafe {
        StrCmpLogicalW(
            PCWSTR(a.as_ptr()),
            PCWSTR(b.as_ptr()),
        )
    };
    result.cmp(&0)
}

/// One row handed back for a client-side re-sort, so changing the sort column does not
/// re-enumerate the folder.
pub struct SortItem {
    pub name: String,
    pub path: Option<String>,
    pub size: u64,
    pub mtime: u64,
    pub is_dir: bool,
}

/// Indices of `items` in display order. Mirrors [`sort_entries`] so a re-sort in the
/// frontend produces exactly the order a fresh scan would.
pub fn sort_order(items: &[SortItem], key: &str, dir: &str) -> Vec<u32> {
    let desc = dir == "desc";
    let wide: Vec<Vec<u16>> = items
        .iter()
        .map(|item| item.name.encode_utf16().chain(std::iter::once(0)).collect())
        .collect();
    let paths: Vec<Vec<u16>> = items
        .iter()
        .map(|item| {
            item.path
                .as_deref()
                .unwrap_or("")
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect()
        })
        .collect();
    let ext: Vec<Vec<u8>> = items
        .iter()
        .map(|item| {
            natural_key(
                item.name
                    .rsplit_once('.')
                    .map(|(_, ext)| ext)
                    .unwrap_or("")
                    .trim_end(),
            )
        })
        .collect();
    let mut order: Vec<u32> = (0..items.len() as u32).collect();
    order.sort_by(|&a, &b| {
        let (x, y) = (&items[a as usize], &items[b as usize]);
        let folder_order = y.is_dir.cmp(&x.is_dir);
        if folder_order != std::cmp::Ordering::Equal {
            return folder_order;
        }
        let primary = match key {
            "size" => x.size.cmp(&y.size),
            "mtime" => x.mtime.cmp(&y.mtime),
            "type" => ext[a as usize].cmp(&ext[b as usize]),
            "path" => logical_wide(&paths[a as usize], &paths[b as usize]),
            _ => logical_wide(&wide[a as usize], &wide[b as usize]),
        };
        let primary = if desc { primary.reverse() } else { primary };
        primary.then_with(|| logical_wide(&wide[a as usize], &wide[b as usize]))
    });
    order
}

#[derive(serde::Serialize, Clone)]
pub struct Drive {
    /// Path handed to the file list: `C:\` for letters, a UNC share or URL for locations.
    pub root: String,
    pub kind: String,
    /// Row caption: `C:` for letters, the shortcut name for network locations.
    pub name: String,
    /// Volume label; empty for unlabeled volumes and network locations.
    pub label: String,
    /// Filesystem name (`NTFS`, `exFAT`, …); empty when the volume cannot be queried.
    pub fs: String,
    /// Total and free bytes; 0 when unknown (locations, empty optical drives, dead shares).
    pub total: u64,
    pub free: u64,
    /// False for media that cannot be read right now, so the UI can say "unavailable"
    /// instead of showing a misleading 0-byte volume.
    pub available: bool,
}

/// Letter drives followed by Explorer's "网络位置" shortcuts.
pub fn drives() -> Vec<Drive> {
    let mut out = letter_drives();
    out.extend(network_locations());
    out
}

#[derive(Default, Clone)]
struct VolumeInfo {
    label: String,
    fs: String,
    total: u64,
    free: u64,
    available: bool,
}

/// Reads label, filesystem and free space. A disconnected share or an empty optical drive
/// fails the first call, which is exactly the "unavailable" signal the list needs.
fn volume_info(root: &str) -> VolumeInfo {
    use windows::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetVolumeInformationW,
    };

    let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
    let mut label = [0u16; 64];
    let mut fs = [0u16; 32];
    let mut serial = 0u32;
    let mut max_component = 0u32;
    let mut flags = 0u32;
    let named = unsafe {
        GetVolumeInformationW(
            PCWSTR(wide.as_ptr()),
            Some(&mut label),
            Some(&mut serial),
            Some(&mut max_component),
            Some(&mut flags),
            Some(&mut fs),
        )
    };
    if named.is_err() {
        return VolumeInfo::default();
    }
    let mut free = 0u64;
    let mut total = 0u64;
    let sized = unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR(wide.as_ptr()),
            None,
            Some(&mut total),
            Some(&mut free),
        )
    };
    VolumeInfo {
        label: utf16_field(&label),
        fs: utf16_field(&fs),
        total: if sized.is_ok() { total } else { 0 },
        free: if sized.is_ok() { free } else { 0 },
        available: true,
    }
}

/// Trimmed UTF-16 buffer contents, stopping at the first NUL.
fn utf16_field(buffer: &[u16]) -> String {
    let len = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..len])
}

fn letter_drives() -> Vec<Drive> {
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
            name: root.trim_end_matches('\\').to_string(),
            root,
            kind: kind.to_string(),
            label: String::new(),
            fs: String::new(),
            total: 0,
            free: 0,
            available: false,
        });
    }
    fill_volumes(&mut out);
    out
}

/// Queries every letter in parallel with a short overall deadline: one dead network share
/// must not stall the whole overview. Results are cached per root and a probe that is still
/// blocked is not started again, so repeated refreshes cannot accumulate threads.
fn fill_volumes(drives: &mut [Drive]) {
    use std::sync::{mpsc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    const DEADLINE: Duration = Duration::from_millis(1200);
    const SUCCESS_TTL: Duration = Duration::from_secs(15);
    const FAILURE_TTL: Duration = Duration::from_secs(60);

    #[derive(Clone)]
    struct Cached {
        info: Option<VolumeInfo>,
        checked: Instant,
        inflight: bool,
    }

    static CACHE: OnceLock<Mutex<std::collections::HashMap<String, Cached>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let now = Instant::now();
    let mut spawn: Vec<String> = Vec::new();
    {
        let mut map = cache.lock().unwrap();
        for drive in drives.iter_mut() {
            let entry = map.entry(drive.root.clone()).or_insert_with(|| Cached {
                info: None,
                checked: now,
                inflight: false,
            });
            if let Some(info) = &entry.info {
                apply_volume(drive, info);
            }
            let ttl = match &entry.info {
                Some(info) if info.available => SUCCESS_TTL,
                _ => FAILURE_TTL,
            };
            let stale =
                entry.info.is_none() || now.saturating_duration_since(entry.checked) >= ttl;
            if stale && !entry.inflight {
                entry.inflight = true;
                spawn.push(drive.root.clone());
            }
        }
    }
    if spawn.is_empty() {
        return;
    }

    let (tx, rx) = mpsc::channel();
    for root in &spawn {
        let root = root.clone();
        let tx = tx.clone();
        std::thread::spawn(move || {
            let info = volume_info(&root);
            if let Some(cache) = CACHE.get() {
                if let Ok(mut map) = cache.lock() {
                    map.insert(
                        root.clone(),
                        Cached {
                            info: Some(info.clone()),
                            checked: Instant::now(),
                            inflight: false,
                        },
                    );
                }
            }
            let _ = tx.send((root, info));
        });
    }
    drop(tx);

    let deadline = Instant::now() + DEADLINE;
    for _ in 0..spawn.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Ok((root, info)) = rx.recv_timeout(remaining) else {
            break;
        };
        let Some(drive) = drives.iter_mut().find(|drive| drive.root == root) else {
            continue;
        };
        apply_volume(drive, &info);
    }
}

fn apply_volume(drive: &mut Drive, info: &VolumeInfo) {
    drive.label = info.label.clone();
    drive.fs = info.fs.clone();
    drive.total = info.total;
    drive.free = info.free;
    drive.available = info.available;
}

/// Shortcuts from `%APPDATA%\Microsoft\Windows\Network Shortcuts`, the per-user
/// folder behind Explorer's "网络位置". Targets may be UNC shares or WebDAV URLs;
/// the list passes both through and the UI opens non-path targets with the shell.
fn network_locations() -> Vec<Drive> {
    let Some(appdata) = std::env::var_os("APPDATA") else {
        return Vec::new();
    };
    let dir = std::path::PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Network Shortcuts");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    // Runs on a blocking-pool thread now, which previews already initialise as MTA;
    // ShellLink is a "Both" object, so the apartment model does not matter. S_FALSE
    // (already initialised) is fine, and the missing CoUninitialize keeps the thread's
    // apartment for later calls on the same thread — same as the icon cache.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let mut out: Vec<Drive> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        // Locations created by Explorer are folders holding `target.lnk`; older
        // entries are loose `.lnk` files named after the location.
        let (name, link) = if path.is_dir() {
            let name = entry.file_name().to_string_lossy().into_owned();
            (name, path.join("target.lnk"))
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")) {
            (path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), path)
        } else {
            continue;
        };
        if name.is_empty() || !link.is_file() {
            continue;
        }
        let Some(root) = shortcut_target(&link) else {
            continue;
        };
        out.push(Drive {
            root,
            kind: "location".to_string(),
            name,
            // Locations have no volume: the UI shows "—" rather than a fake 0 bytes.
            label: String::new(),
            fs: String::new(),
            total: 0,
            free: 0,
            available: false,
        });
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

/// Reads the target stored inside a `.lnk` without resolving it, so listing network
/// locations never blocks on an unreachable server.
fn shortcut_target(lnk: &Path) -> Option<String> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        let path = lnk.to_string_lossy();
        file.Load(&HSTRING::from(path.as_ref()), STGM_READ).ok()?;
        let mut buffer = [0u16; 1024];
        link.GetPath(&mut buffer, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)
            .ok()?;
        let len = buffer.iter().position(|c| *c == 0)?;
        let target = String::from_utf16_lossy(&buffer[..len]);
        (!target.is_empty()).then_some(target)
    }
}

#[derive(serde::Serialize, Clone)]
pub struct Place {
    pub key: String,
    pub path: String,
}

/// Resolves a known folder, freeing the shell-allocated string. Empty results are dropped.
fn known_folder(id: &windows::core::GUID) -> Option<String> {
    use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT};

    unsafe {
        let raw = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let path = raw.to_string().ok();
        CoTaskMemFree(Some(raw.0 as *const c_void));
        path.filter(|path| !path.is_empty())
    }
}

pub fn quick_access() -> Vec<Place> {
    // OneDrive and enterprise redirection move Desktop/Documents out of `%USERPROFILE%`,
    // so the shell's known folders are authoritative; the env var stays as the fallback
    // for environments where the lookup itself fails.
    let home = known_folder(&FOLDERID_Profile)
        .or_else(|| std::env::var("USERPROFILE").ok().filter(|home| !home.is_empty()));
    let Some(home) = home else {
        return Vec::new();
    };
    let mut out = vec![Place {
        key: "home".to_string(),
        path: home.clone(),
    }];
    let mut seen = std::collections::HashSet::from([home.to_lowercase()]);
    for (key, folder, sub) in [
        ("desktop", &FOLDERID_Desktop, "Desktop"),
        ("downloads", &FOLDERID_Downloads, "Downloads"),
        ("documents", &FOLDERID_Documents, "Documents"),
    ] {
        let path = known_folder(folder).unwrap_or_else(|| {
            Path::new(&home)
                .join(sub)
                .to_string_lossy()
                .into_owned()
        });
        // A redirected folder may coincide with another entry; one row per path is enough.
        if !seen.insert(path.to_lowercase()) {
            continue;
        }
        if Path::new(&path).exists() {
            out.push(Place {
                key: key.to_string(),
                path,
            });
        }
    }
    out
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    /// Guards the `dirs_only` filter: a `continue` inside the record loop would skip
    /// the offset advance and spin on the same entry forever.
    #[test]
    fn dirs_only_returns_folders_and_terminates() {
        let root = std::env::temp_dir().join(format!("flowdir-scan-{}", std::process::id()));
        std::fs::create_dir_all(root.join("alpha")).unwrap();
        std::fs::create_dir_all(root.join("beta")).unwrap();
        std::fs::write(root.join("file.txt"), b"x").unwrap();

        let cancel = AtomicBool::new(false);
        let mut dirs = enumerate_dirs(root.to_str().unwrap(), true, 64, &cancel)
            .unwrap()
            .entries;
        sort_entries(&mut dirs, "name", "asc");
        assert_eq!(
            dirs.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(),
            ["alpha", "beta"]
        );

        let mut all = enumerate_reporting(root.to_str().unwrap(), true, &cancel, &mut |_| {})
            .unwrap()
            .entries;
        sort_entries(&mut all, "name", "asc");
        assert_eq!(all.len(), 3);

        std::fs::remove_dir_all(root).unwrap();
    }

    /// The SMB fallback must produce the same rows and honour the same cap as the fast path.
    #[test]
    fn fallback_matches_the_fast_scan() {
        let root = std::env::temp_dir().join(format!("flowdir-scan-fallback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("alpha")).unwrap();
        std::fs::write(root.join("one.txt"), b"1234567890").unwrap();
        std::fs::write(root.join("two.txt"), b"xy").unwrap();

        let cancel = AtomicBool::new(false);
        let fast = scan(root.to_str().unwrap(), true, false, 100, &cancel, &mut |_| {}).unwrap();
        let fallback =
            scan_fallback(root.to_str().unwrap(), true, false, 100, &cancel, &mut |_| {}).unwrap();
        let mut fast_rows: Vec<_> = fast
            .entries
            .iter()
            .map(|entry| (entry.name.clone(), entry.size, entry.is_dir()))
            .collect();
        let mut fallback_rows: Vec<_> = fallback
            .entries
            .iter()
            .map(|entry| (entry.name.clone(), entry.size, entry.is_dir()))
            .collect();
        fast_rows.sort();
        fallback_rows.sort();
        assert_eq!(fast_rows, fallback_rows);
        assert!(!fast.truncated && !fallback.truncated);

        let capped =
            scan_fallback(root.to_str().unwrap(), true, false, 2, &cancel, &mut |_| {}).unwrap();
        assert_eq!(capped.entries.len(), 2);
        assert!(capped.truncated);

        std::fs::remove_dir_all(root).unwrap();
    }

    /// Folders first, digit runs numerically, and the same order a fresh scan would give.
    #[test]
    fn sort_order_matches_the_scanner() {
        let items = vec![
            SortItem { name: "file10.txt".into(), path: Some(r"C:\b\file10.txt".into()), size: 10, mtime: 3, is_dir: false },
            SortItem { name: "file2.txt".into(), path: Some(r"C:\a\file2.txt".into()), size: 20, mtime: 2, is_dir: false },
            SortItem { name: "Docs".into(), path: Some(r"C:\c\Docs".into()), size: 0, mtime: 1, is_dir: true },
            SortItem { name: "中文.txt".into(), path: Some(r"C:\a\中文.txt".into()), size: 30, mtime: 4, is_dir: false },
        ];
        let names = |order: Vec<u32>| {
            order
                .into_iter()
                .map(|i| items[i as usize].name.clone())
                .collect::<Vec<_>>()
        };
        let by_name = names(sort_order(&items, "name", "asc"));
        assert_eq!(by_name[0], "Docs");
        assert_eq!(&by_name[1..3], ["file2.txt", "file10.txt"]);
        assert_eq!(by_name.len(), 4);

        let by_size = names(sort_order(&items, "size", "asc"));
        assert_eq!(by_size[0], "Docs");
        assert_eq!(&by_size[1..], ["file10.txt", "file2.txt", "中文.txt"]);

        let by_path = names(sort_order(&items, "path", "asc"));
        assert_eq!(by_path[0], "Docs");
        assert_eq!(&by_path[1..3], ["file2.txt", "中文.txt"]);

        // A descending sort flips the key but keeps folders on top, like Explorer.
        let desc = names(sort_order(&items, "name", "desc"));
        assert_eq!(desc[0], "Docs");
    }
    /// The overview must show real capacity for a system drive and mark dead devices as
    /// unavailable rather than as 0-byte volumes.
    #[test]
    fn volume_info_reports_capacity_and_failures() {
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let info = volume_info(&format!("{system}\\"));
        assert!(info.available, "system volume must be readable");
        assert!(info.total > 0);
        assert!(info.free <= info.total);

        let dead = volume_info("\\\\localhost\\flowdir-no-such-share");
        assert!(!dead.available);
        assert_eq!(dead.total, 0);
    }

    /// The tree cap must be reported, and progress must fire before the sort so a slow
    /// volume can grow the counter instead of leaving the pane on a spinner.
    #[test]
    fn scan_reports_truncation_and_progress() {
        let root = std::env::temp_dir().join(format!("flowdir-scan-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for name in ["a", "b", "c"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let full = enumerate_reporting(root.to_str().unwrap(), true, &cancel, &mut |count| {
            seen.push(count)
        })
        .unwrap();
        assert_eq!(full.entries.len(), 3);
        assert!(!full.truncated);
        assert_eq!(seen.last().copied(), Some(3), "progress must report the read count");

        let capped = enumerate_dirs(root.to_str().unwrap(), true, 2, &cancel).unwrap();
        assert!(capped.truncated, "the row cap must be visible to callers");
        assert_eq!(capped.entries.len(), 2);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Known folders are resolved by the shell, so a redirected Desktop is the one that
    /// appears; every row must be a real folder and no two rows may share a path.
    #[test]
    fn quick_access_uses_known_folders() {
        let places = quick_access();
        let home = known_folder(&FOLDERID_Profile).expect("profile folder");
        assert_eq!(places[0].key, "home");
        assert!(places[0].path.eq_ignore_ascii_case(&home));

        let mut seen = std::collections::HashSet::new();
        for place in &places {
            assert!(!place.path.is_empty(), "empty path for {}", place.key);
            assert!(Path::new(&place.path).exists(), "missing {}", place.path);
            assert!(seen.insert(place.path.to_lowercase()), "duplicate {}", place.path);
        }
        for (key, folder) in [
            ("desktop", &FOLDERID_Desktop),
            ("downloads", &FOLDERID_Downloads),
            ("documents", &FOLDERID_Documents),
        ] {
            let Some(path) = known_folder(folder) else {
                continue;
            };
            assert!(
                places.iter().any(|place| place.key == key && place.path.eq_ignore_ascii_case(&path)),
                "{key} must use {path}"
            );
        }
    }
}
