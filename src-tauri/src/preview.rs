use std::collections::HashMap;
use std::path::Path as FsPath;
use std::sync::Mutex;

use serde::Serialize;
use tauri::http::{header, Response, StatusCode};
use tauri::{AppHandle, Manager, Runtime};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Storage::StorageFile;
use windows::Storage::Streams::{DataReader, InMemoryRandomAccessStream};
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGR, GUID_WICPixelFormat32bppBGRA, IWICBitmap,
    IWICBitmapScaler, IWICBitmapSource, IWICFormatConverter, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSourceReader, MFCreateAttributes, MFCreateMediaType,
    MFCreateSourceReaderFromURL, MFMediaType_Video, MFShutdown, MFStartup, MFVideoFormat_RGB32,
    MFSTARTUP_FULL, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_VERSION,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

use crate::error::ApiError;
use crate::settings::{ExternalPreview, Settings};

/// Icon and thumbnail payloads are small and cheap to regenerate; a generous cache
/// only inflates the resident set of a file manager that is meant to stay small.
const MAX_CACHE_BYTES: usize = 12 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 50;

/// Cloud-provider attributes. A file carrying any of them is a placeholder whose bytes
/// live only in the cloud; touching it would download it, which the preview must not do
/// until the user asks for it explicitly.
const ATTR_OFFLINE: u32 = 0x0000_1000;
const ATTR_RECALL_ON_OPEN: u32 = 0x0004_0000;
const ATTR_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

/// Hard ceiling for one decoded preview bitmap (32bpp BGRA). A 4096×4096 image is 64 MB,
/// which is the largest frame the panel can show anyway.
const MAX_RENDER_BYTES: usize = 64 * 1024 * 1024;

/// Caps applied to every preview read. They come from settings so a slow volume or a
/// very large PDF can be tuned without a rebuild.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_bytes: u64,
    pub max_dimension: u32,
    pub text_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 64 * 1024 * 1024,
            max_dimension: 4096,
            text_bytes: 8 * 1024,
        }
    }
}

impl Limits {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            max_bytes: u64::from(settings.preview_max_file_mb) * 1024 * 1024,
            max_dimension: settings.preview_max_dimension,
            text_bytes: settings.preview_text_kb as usize * 1024,
        }
    }

    /// Width a rendered image/PDF may use; never above the configured pixel cap, so a
    /// dragged-out panel cannot ask for a bitmap larger than the limit.
    pub fn width(&self, requested: u32) -> u32 {
        requested.clamp(64, self.max_dimension.max(64))
    }
}

const IMAGE_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff", "ico", "heic", "avif",
];

const VIDEO_EXT: &[&str] = &[
    "mp4", "m4v", "mkv", "mov", "avi", "webm", "wmv", "flv", "mpg", "mpeg", "ts", "m2ts",
];

/// Extensions always treated as text. Anything else is sniffed, so an unlisted or
/// extension-less file still previews when its bytes look like text.
const TEXT_EXT: &[&str] = &[
    "txt", "md", "markdown", "json", "jsonc", "js", "jsx", "mjs", "cjs", "ts", "tsx", "css",
    "scss", "sass", "less", "html", "htm", "xml", "yaml", "yml", "toml", "ini", "cfg", "conf",
    "properties", "log", "csv", "tsv", "rs", "py", "go", "java", "kt", "kts", "c", "h", "cc",
    "cpp", "cxx", "hpp", "hxx", "cs", "rb", "php", "pl", "lua", "swift", "dart", "scala", "r",
    "sql", "sh", "zsh", "fish", "bat", "cmd", "ps1", "vue", "svelte", "astro", "tex", "proto",
    "graphql", "gql", "gradle", "dockerfile", "makefile", "cmake", "diff", "patch", "srt", "vtt",
    "reg", "inf", "gitignore", "gitattributes", "editorconfig", "env", "lock",
];

#[derive(Default)]
pub struct PreviewCache {
    inner: Mutex<CacheInner>,
}

#[derive(Default)]
struct CacheInner {
    entries: HashMap<String, (Vec<u8>, u64)>,
    total: usize,
    tick: u64,
}

impl PreviewCache {
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let mut inner = self.inner.lock().unwrap();
        inner.tick += 1;
        let tick = inner.tick;
        let entry = inner.entries.get_mut(key)?;
        entry.1 = tick;
        Some(entry.0.clone())
    }

    pub fn put(&self, key: String, data: Vec<u8>) {
        let mut inner = self.inner.lock().unwrap();
        inner.tick += 1;
        let tick = inner.tick;
        if let Some((old, _)) = inner.entries.remove(&key) {
            inner.total = inner.total.saturating_sub(old.len());
        }
        inner.total += data.len();
        inner.entries.insert(key, (data, tick));
        while inner.total > MAX_CACHE_BYTES && !inner.entries.is_empty() {
            let oldest = inner
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| k.clone());
            if let Some(key) = oldest {
                if let Some((old, _)) = inner.entries.remove(&key) {
                    inner.total = inner.total.saturating_sub(old.len());
                }
            } else {
                break;
            }
        }
    }
}

#[derive(Serialize, Clone)]
pub struct ArchiveEntry {
    pub name: String,
    pub size: u64,
}

#[derive(Serialize, Clone)]
pub struct MetaRow {
    pub label: String,
    pub value: String,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct PreviewInfo {
    pub kind: String,
    pub url: Option<String>,
    pub text: Option<String>,
    pub entries: Option<Vec<ArchiveEntry>>,
    pub meta: Option<Vec<MetaRow>>,
    pub reason: Option<String>,
    /// Caption of the configured external preview program, when one handles this extension.
    pub external: Option<String>,
    pub size: u64,
    pub mtime: u64,
}

fn extension(path: &str) -> String {
    FsPath::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// Windows device names are reserved even with an extension (`CON.txt`).
const RESERVED_NAMES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Rejects NTFS alternate data streams (`file.txt:stream`, `::$DATA`) and device names.
/// The drive-letter colon at index 1 is the only one a normal path may contain.
fn has_ads_or_device(path: &str) -> bool {
    for (index, ch) in path.char_indices() {
        if ch == ':' && index != 1 {
            return true;
        }
    }
    for segment in path.split(['\\', '/']) {
        if segment.is_empty() || segment == "." || segment == ".." {
            continue;
        }
        if segment.ends_with(' ') || segment.ends_with('.') {
            return true;
        }
        let stem = segment
            .split('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if RESERVED_NAMES.contains(&stem.as_str()) {
            return true;
        }
    }
    false
}

/// True when the bytes for this file are still in the cloud. Reading such a file would
/// pull the whole thing down, so the preview stops at a "download first" state instead.
fn is_cloud_placeholder(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & (ATTR_OFFLINE | ATTR_RECALL_ON_OPEN | ATTR_RECALL_ON_DATA_ACCESS) != 0
}

fn validate(path: &str, limits: Limits) -> Result<(), ApiError> {
    if path.is_empty() || path.len() > 1024 {
        return Err(ApiError::new("INVALID_PATH", "path length"));
    }
    let p = FsPath::new(path);
    if !p.is_absolute() {
        return Err(ApiError::new("INVALID_PATH", "path must be absolute"));
    }
    if path.contains('\0') {
        return Err(ApiError::new("INVALID_PATH", "nul byte"));
    }
    if has_ads_or_device(path) {
        return Err(ApiError::new("INVALID_PATH", "ads or device name"));
    }
    // Resolve `..`, symlinks and 8.3 aliases before touching the file so the final
    // target is what gets validated, not the lexical spelling.
    let canonical = std::fs::canonicalize(p).map_err(|e| ApiError::from_io(path, &e))?;
    if has_ads_or_device(&canonical.to_string_lossy()) {
        return Err(ApiError::new("INVALID_PATH", "ads or device name"));
    }
    let meta =
        std::fs::metadata(&canonical).map_err(|e| ApiError::from_io(path, &e))?;
    if meta.is_dir() {
        return Err(ApiError::new("IS_DIR", path));
    }
    if is_cloud_placeholder(&meta) {
        // The metadata call above does not hydrate; only a data read would.
        return Err(ApiError::new("CLOUD_PLACEHOLDER", path));
    }
    if meta.len() > limits.max_bytes {
        return Err(ApiError::new("TOO_LARGE", path));
    }
    Ok(())
}

fn ensure_com() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

/// HTTP response for `http://fd.localhost/preview?...`.
pub fn handle_request<R: Runtime>(app: &AppHandle<R>, uri: &tauri::http::Uri) -> Response<Vec<u8>> {
    let params: HashMap<String, String> = uri
        .query()
        .map(|q| url_form(q).into_iter().collect::<HashMap<_, _>>())
        .unwrap_or_default();

    let Some(path) = params.get("path") else {
        return status(StatusCode::BAD_REQUEST);
    };
    let limits = Limits::from_settings(&app.state::<crate::settings::SettingsCache>().get(app));
    if let Err(err) = validate(path, limits) {
        // 409 tells the <img> nothing, but the frontend never builds a URL for a
        // placeholder: it shows the "download first" state instead.
        return plain(StatusCode::CONFLICT, &err.code);
    }
    let width = limits.width(
        params
            .get("w")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(512),
    );
    let bg = params
        .get("bg")
        .and_then(|v| u32::from_str_radix(v, 16).ok())
        .unwrap_or(0xffffff);

    let key = uri
        .query()
        .map(|q| q.to_string())
        .unwrap_or_else(|| path.clone());
    let cache = app.state::<PreviewCache>();

    if let Some(bytes) = cache.get(&key) {
        return ok(
            if extension(path) == "pdf" {
                "image/png"
            } else {
                "image/bmp"
            },
            bytes,
        );
    }

    match extension(path).as_str() {
        ext if IMAGE_EXT.contains(&ext) => match render_image(path, width, bg) {
            Ok(bytes) => {
                cache.put(key, bytes.clone());
                ok("image/bmp", bytes)
            }
            Err(err) => plain(StatusCode::INTERNAL_SERVER_ERROR, &err.code),
        },
        ext if VIDEO_EXT.contains(&ext) => match render_video(path, width, bg) {
            Ok(bytes) => {
                cache.put(key, bytes.clone());
                ok("image/bmp", bytes)
            }
            Err(err) => plain(StatusCode::INTERNAL_SERVER_ERROR, &err.code),
        },
        "pdf" => match render_pdf(path, width) {
            Ok(bytes) => {
                cache.put(key, bytes.clone());
                ok("image/png", bytes)
            }
            Err(err) => plain(StatusCode::INTERNAL_SERVER_ERROR, &err.code),
        },
        _ => plain(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported"),
    }
}

fn ok(content_type: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(body)
        .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR))
}

fn plain(status_code: StatusCode, message: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status_code)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(message.as_bytes().to_vec())
        .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR))
}

fn status(status_code: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(status_code)
        .body(Vec::new())
        .unwrap()
}

fn url_form(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            let decode = |raw: &str| {
                percent_encoding::percent_decode_str(raw)
                    .decode_utf8_lossy()
                    .into_owned()
            };
            Some((decode(key), decode(value)))
        })
        .collect()
}

pub fn info(
    path: String,
    width: u32,
    bg: u32,
    external: &[ExternalPreview],
    limits: Limits,
    hydrate: bool,
) -> Result<PreviewInfo, ApiError> {
    let meta = std::fs::metadata(&path).map_err(|e| ApiError::from_io(&path, &e))?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    if meta.is_dir() {
        return Ok(PreviewInfo {
            kind: "folder".into(),
            mtime,
            ..Default::default()
        });
    }
    let size = meta.len();
    let ext = extension(&path);
    let external = rule_for(&ext, external).map(|rule| command_label(&rule.command));
    // The metadata read does not touch the data stream; only the explicit hydrate path
    // does, so a cloud file is reported as a placeholder until the user asks for it.
    if is_cloud_placeholder(&meta) {
        if !hydrate {
            return Ok(PreviewInfo {
                kind: "cloud".into(),
                reason: Some("cloud-placeholder".into()),
                size,
                mtime,
                external,
                ..Default::default()
            });
        }
        bring_online(&path)?;
    }
    if size > limits.max_bytes {
        return Ok(PreviewInfo {
            kind: "unsupported".into(),
            reason: Some("too-large".into()),
            size,
            mtime,
            external,
            ..Default::default()
        });
    }
    let mut info = PreviewInfo {
        size,
        mtime,
        external,
        ..Default::default()
    };

    let width = limits.width(width);
    if IMAGE_EXT.contains(&ext.as_str()) || ext == "pdf" || VIDEO_EXT.contains(&ext.as_str()) {
        let encoded =
            percent_encoding::utf8_percent_encode(&path, percent_encoding::NON_ALPHANUMERIC);
        info.kind = if ext == "pdf" {
            "pdf"
        } else if VIDEO_EXT.contains(&ext.as_str()) {
            "video"
        } else {
            "image"
        }
        .to_string();
        info.url = Some(format!(
            "http://fd.localhost/preview?path={encoded}&w={width}&bg={bg:06x}&v={mtime}-{size}"
        ));
        return Ok(info);
    }

    match ext.as_str() {
        "zip" => {
            let file = std::fs::File::open(&path).map_err(|e| ApiError::from_io(&path, &e))?;
            let mut archive =
                zip::ZipArchive::new(file).map_err(|e| ApiError::new("ARCHIVE", e.to_string()))?;
            let mut entries = Vec::new();
            for index in 0..archive.len().min(MAX_ARCHIVE_ENTRIES) {
                if let Ok(entry) = archive.by_index(index) {
                    entries.push(ArchiveEntry {
                        name: entry.name().map(|n| n.to_string()).unwrap_or_default(),
                        size: entry.size(),
                    });
                }
            }
            info.kind = "archive".to_string();
            info.entries = Some(entries);
        }
        "exe" | "dll" | "sys" | "msi" => {
            info.kind = "binary".to_string();
            info.meta = Some(version_rows(&path));
        }
        _ => {
            let known = TEXT_EXT.contains(&ext.as_str());
            match read_prefix(&path, limits.text_bytes) {
                // Sniffing keeps unknown and extension-less text (Lua scripts, Dockerfile,
                // LICENSE, …) previewable without an ever-growing extension table.
                Ok(bytes) if known || looks_like_text(&bytes) => {
                    info.kind = "text".to_string();
                    info.text = Some(decode_text(&bytes));
                }
                // A whitelisted text file must report why it could not be read; for an
                // unknown binary the failed read simply means "no preview".
                Err(err) if known => return Err(err),
                _ => {
                    info.kind = "unsupported".to_string();
                    info.reason = Some("no-preview".to_string());
                }
            }
        }
    }
    Ok(info)
}

/// Head of the file, capped at the configured text size: a text preview never needs more
/// and reading a 64 MB log in full would stall the reply.
fn read_prefix(path: &str, max: usize) -> Result<Vec<u8>, ApiError> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| ApiError::from_io(path, &e))?;
    let mut bytes = Vec::with_capacity(max);
    file.take(max as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| ApiError::from_io(path, &e))?;
    Ok(bytes)
}

/// Reads one byte of the file, which is what makes a cloud provider fetch the data.
/// It returns once the first bytes have arrived; the provider keeps streaming the rest
/// in the background, so a large file does not block the preview for its full size.
fn bring_online(path: &str) -> Result<(), ApiError> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| ApiError::from_io(path, &e))?;
    let mut probe = [0u8; 1];
    file.read(&mut probe)
        .map_err(|e| ApiError::from_io(path, &e))?;
    Ok(())
}

/// Binary sniff: NUL bytes or more than 2% control characters mean the prefix is not
/// readable text. Files with a text BOM are always accepted, even UTF-16 where every
/// ASCII character carries a NUL.
fn looks_like_text(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF])
        || bytes.starts_with(&[0xFF, 0xFE])
        || bytes.starts_with(&[0xFE, 0xFF])
    {
        return true;
    }
    let mut control = 0usize;
    for &byte in bytes {
        match byte {
            0x00 => return false,
            0x09 | 0x0A | 0x0C | 0x0D => {}
            0x01..=0x1F | 0x7F => control += 1,
            _ => {}
        }
    }
    control * 100 <= bytes.len() * 2
}

/// First matching rule for `ext`; entries without a command are ignored so a half-typed
/// rule in settings.toml does not shadow anything.
fn rule_for<'a>(ext: &str, external: &'a [ExternalPreview]) -> Option<&'a ExternalPreview> {
    external
        .iter()
        .find(|rule| rule.ext.eq_ignore_ascii_case(ext) && !rule.command.trim().is_empty())
}

/// Caption for the preview button: the program's file stem (`WINWORD`, `notepad`).
fn command_label(command: &str) -> String {
    split_command(command)
        .first()
        .and_then(|program| FsPath::new(program).file_stem())
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or(command)
        .to_string()
}

/// Splits a command line into program and arguments. Double quotes group one token;
/// there are no escape sequences, matching how a path with spaces is typed.
fn split_command(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut started = false;
    for ch in command.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    tokens.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        tokens.push(current);
    }
    tokens
}

/// Starts the configured preview program for `path` as a detached child process.
pub fn launch_external(path: &str, external: &[ExternalPreview]) -> Result<(), ApiError> {
    let ext = extension(path);
    let rule = rule_for(&ext, external).ok_or_else(|| ApiError::new("NO_RULE", ext))?;
    let mut argv = split_command(&rule.command);
    if argv.is_empty() {
        return Err(ApiError::new("NO_RULE", rule.command.clone()));
    }
    let mut substituted = false;
    for arg in &mut argv {
        if arg.contains("{path}") {
            *arg = arg.replace("{path}", path);
            substituted = true;
        }
    }
    if !substituted {
        argv.push(path.to_string());
    }
    std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .spawn()
        .map(|_| ())
        .map_err(|e| ApiError::new("LAUNCH", format!("{}: {e}", argv[0])))
}

/// Decodes a text prefix: BOM (UTF-8/UTF-16) first, then strict UTF-8, then the
/// ANSI code page (936 on zh-CN Windows), which is what Notepad-created files use.
fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(rest, false);
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        // The 8 KB cut can split a character: keep the valid prefix instead of
        // mis-detecting the whole file as ANSI.
        Err(err) if err.error_len().is_none() => {
            String::from_utf8_lossy(&bytes[..err.valid_up_to()]).into_owned()
        }
        Err(_) => {
            ansi_to_string(bytes).unwrap_or_else(|| String::from_utf8_lossy(bytes).into_owned())
        }
    }
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| {
            if little_endian {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

/// Non-UTF-8 text: try the ANSI code page, then GBK explicitly. `CP_ACP` is
/// UTF-8 on systems with the "beta: UTF-8" locale option, so 936 must be tried
/// on its own instead of trusting ACP.
fn ansi_to_string(bytes: &[u8]) -> Option<String> {
    use windows::Win32::Globalization::{
        GetACP, MultiByteToWideChar, MULTI_BYTE_TO_WIDE_CHAR_FLAGS,
    };
    const CP_UTF8: u32 = 65001;
    const CP_GBK: u32 = 936;
    if bytes.is_empty() {
        return Some(String::new());
    }
    let acp = unsafe { GetACP() };
    let mut candidates = Vec::with_capacity(2);
    if acp != CP_UTF8 {
        candidates.push(acp);
    }
    if acp != CP_GBK {
        candidates.push(CP_GBK);
    }
    for codepage in candidates {
        // MB_ERR_INVALID_CHARS rejects byte sequences the code page cannot map,
        // which keeps random binary from being shown as text.
        let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0x0000_0008);
        unsafe {
            let needed = MultiByteToWideChar(codepage, flags, bytes, None);
            if needed <= 0 {
                continue;
            }
            let mut wide = vec![0u16; needed as usize];
            let written = MultiByteToWideChar(codepage, flags, bytes, Some(&mut wide));
            if written <= 0 {
                continue;
            }
            wide.truncate(written as usize);
            return Some(String::from_utf16_lossy(&wide));
        }
    }
    None
}

fn render_image(path: &str, max_width: u32, bg: u32) -> Result<Vec<u8>, ApiError> {
    ensure_com();
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| ApiError::new("WIC", e.to_string()))?;
        let decoder = factory
            .CreateDecoderFromFilename(
                PCWSTR(wide.as_ptr()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .map_err(|e| ApiError::new("WIC", e.to_string()))?;
        let frame = decoder
            .GetFrame(0)
            .map_err(|e| ApiError::new("WIC", e.to_string()))?;

        let (target_w, target_h, pixels, stride) = scale_to_bgra(&factory, &frame, max_width)?;
        Ok(bgra_to_bmp(target_w, target_h, &pixels, stride, Some(bg)))
    }
}

/// Scales any WIC source down to `max_width` and converts it to top-down BGRA.
fn scale_to_bgra(
    factory: &IWICImagingFactory,
    source: &IWICBitmapSource,
    max_width: u32,
) -> Result<(u32, u32, Vec<u8>, u32), ApiError> {
    unsafe {
        let mut width = 0u32;
        let mut height = 0u32;
        source
            .GetSize(&mut width, &mut height)
            .map_err(|e| ApiError::new("WIC", e.to_string()))?;
        if width == 0 || height == 0 {
            return Err(ApiError::new("WIC", "empty image"));
        }
        let scale = (max_width as f64 / width as f64).min(1.0);
        let target_w = ((width as f64 * scale).round() as u32).max(1);
        let target_h = ((height as f64 * scale).round() as u32).max(1);
        // A malformed header can claim enormous dimensions; refuse before allocating so
        // the multiplication below cannot wrap (release) or panic (debug).
        let stride = target_w
            .checked_mul(4)
            .ok_or_else(|| ApiError::new("IMAGE_TOO_LARGE", "stride overflow"))?;
        let total = stride
            .checked_mul(target_h)
            .and_then(|bytes| usize::try_from(bytes).ok())
            .filter(|bytes| *bytes <= MAX_RENDER_BYTES)
            .ok_or_else(|| ApiError::new("IMAGE_TOO_LARGE", "pixels overflow"))?;

        let scaler: IWICBitmapScaler = factory
            .CreateBitmapScaler()
            .map_err(|e| ApiError::new("WIC", e.to_string()))?;
        scaler
            .Initialize(source, target_w, target_h, WICBitmapInterpolationModeFant)
            .map_err(|e| ApiError::new("WIC", e.to_string()))?;

        let converter: IWICFormatConverter = factory
            .CreateFormatConverter()
            .map_err(|e| ApiError::new("WIC", e.to_string()))?;
        converter
            .Initialize(
                &scaler,
                &GUID_WICPixelFormat32bppBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(|e| ApiError::new("WIC", e.to_string()))?;

        let stride = target_w * 4;
        let mut pixels = vec![0u8; total];
        converter
            .CopyPixels(std::ptr::null(), stride, &mut pixels)
            .map_err(|e| ApiError::new("WIC", e.to_string()))?;
        Ok((target_w, target_h, pixels, stride))
    }
}

/// First decodable video frame, grabbed ~1s in to skip black intros.
fn render_video(path: &str, max_width: u32, bg: u32) -> Result<Vec<u8>, ApiError> {
    ensure_com();
    unsafe {
        MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|e| ApiError::new("MF", e.to_string()))?;
        let result = video_frame(path, max_width, bg);
        let _ = MFShutdown();
        result
    }
}

fn video_frame(path: &str, max_width: u32, bg: u32) -> Result<Vec<u8>, ApiError> {
    unsafe {
        let mut attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attributes, 1).map_err(|e| ApiError::new("MF", e.to_string()))?;
        let attributes = attributes.ok_or_else(|| ApiError::new("MF", "attributes"))?;
        attributes
            .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
            .map_err(|e| ApiError::new("MF", e.to_string()))?;

        let reader: IMFSourceReader =
            MFCreateSourceReaderFromURL(&HSTRING::from(path), &attributes)
                .map_err(|e| ApiError::new("MF", e.to_string()))?;
        let media_type = MFCreateMediaType().map_err(|e| ApiError::new("MF", e.to_string()))?;
        media_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| ApiError::new("MF", e.to_string()))?;
        media_type
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
            .map_err(|e| ApiError::new("MF", e.to_string()))?;
        reader
            .SetCurrentMediaType(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                None,
                &media_type,
            )
            .map_err(|e| ApiError::new("MF", e.to_string()))?;

        // Best effort: 1 second in, so the frame is not a black fade-in.
        let mut position: windows::Win32::System::Com::StructuredStorage::PROPVARIANT =
            std::mem::zeroed();
        position.Anonymous.Anonymous = std::mem::ManuallyDrop::new(
            windows::Win32::System::Com::StructuredStorage::PROPVARIANT_0_0 {
                vt: windows::Win32::System::Variant::VT_I8,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: windows::Win32::System::Com::StructuredStorage::PROPVARIANT_0_0_0 {
                    hVal: 10_000_000,
                },
            },
        );
        let _ = reader.SetCurrentPosition(&windows::core::GUID::zeroed(), &position);

        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        let mut frame: Option<(u32, u32, Vec<u8>)> = None;
        for _ in 0..8 {
            let mut flags = 0u32;
            let mut timestamp = 0i64;
            let mut sample = None;
            reader
                .ReadSample(
                    stream,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut timestamp),
                    Some(&mut sample),
                )
                .map_err(|e| ApiError::new("MF", e.to_string()))?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            let Some(sample) = sample else { continue };
            let current = reader
                .GetCurrentMediaType(stream)
                .map_err(|e| ApiError::new("MF", e.to_string()))?;
            let packed = current
                .GetUINT64(&MF_MT_FRAME_SIZE)
                .map_err(|e| ApiError::new("MF", e.to_string()))?;
            let width = (packed >> 32) as u32;
            let height = (packed & 0xFFFF_FFFF) as u32;
            if width == 0 || height == 0 {
                continue;
            }
            let stride = current
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .map(|value| value as i32)
                .unwrap_or(-(width as i32 * 4));
            let row_bytes = width as usize * 4;
            let abs_stride = stride.unsigned_abs() as usize;
            if abs_stride < row_bytes {
                continue;
            }
            let buffer = sample
                .ConvertToContiguousBuffer()
                .map_err(|e| ApiError::new("MF", e.to_string()))?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut length = 0u32;
            buffer
                .Lock(&mut ptr, None, Some(&mut length))
                .map_err(|e| ApiError::new("MF", e.to_string()))?;
            let available = length as usize;
            let needed = (height as usize - 1) * abs_stride + row_bytes;
            let mut pixels = Vec::with_capacity(height as usize * row_bytes);
            if !ptr.is_null() && available >= needed {
                let source = std::slice::from_raw_parts(ptr, available);
                for row in 0..height as usize {
                    let src_row = if stride < 0 {
                        height as usize - 1 - row
                    } else {
                        row
                    };
                    let start = src_row * abs_stride;
                    pixels.extend_from_slice(&source[start..start + row_bytes]);
                }
            }
            let _ = buffer.Unlock();
            if !pixels.is_empty() {
                frame = Some((width, height, pixels));
                break;
            }
        }

        let Some((width, height, pixels)) = frame else {
            return Err(ApiError::new("MF", "no frame"));
        };
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| ApiError::new("WIC", e.to_string()))?;
        let bitmap: IWICBitmap = factory
            .CreateBitmapFromMemory(
                width,
                height,
                &GUID_WICPixelFormat32bppBGR,
                width * 4,
                &pixels,
            )
            .map_err(|e| ApiError::new("WIC", e.to_string()))?;
        let (target_w, target_h, scaled, stride) = scale_to_bgra(&factory, &bitmap, max_width)?;
        Ok(bgra_to_bmp(target_w, target_h, &scaled, stride, Some(bg)))
    }
}

/// Packs top-down BGRA pixels into a bottom-up 32bpp BMP. When `bg` is set the
/// alpha channel is composited onto that colour (screenshots/preview), otherwise
/// alpha is discarded (video frames are opaque).
fn bgra_to_bmp(width: u32, height: u32, pixels: &[u8], stride: u32, bg: Option<u32>) -> Vec<u8> {
    let (br, bgc, bb) = match bg {
        Some(bg) => ((bg >> 16) as u8, (bg >> 8) as u8, (bg & 0xff) as u8),
        None => (0, 0, 0),
    };
    let mut bmp = Vec::with_capacity(54 + pixels.len());
    let file_size = 54 + (stride * height) as u32;
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&file_size.to_le_bytes());
    bmp.extend_from_slice(&[0, 0, 0, 0]);
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(width as i32).to_le_bytes());
    bmp.extend_from_slice(&(height as i32).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&32u16.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&(stride * height).to_le_bytes());
    bmp.extend_from_slice(&[0u8; 16]);

    for row in (0..height).rev() {
        let start = (row * stride) as usize;
        for px in pixels[start..start + stride as usize].chunks_exact(4) {
            if bg.is_some() {
                let alpha = px[3] as u32;
                let blend = |channel: u8, base: u8| -> u8 {
                    if alpha == 255 {
                        channel
                    } else {
                        (((channel as u32 * alpha) + (base as u32 * (255 - alpha))) / 255) as u8
                    }
                };
                bmp.push(blend(px[0], bb));
                bmp.push(blend(px[1], bgc));
                bmp.push(blend(px[2], br));
                bmp.push(0);
            } else {
                bmp.push(px[0]);
                bmp.push(px[1]);
                bmp.push(px[2]);
                bmp.push(0);
            }
        }
    }
    bmp
}

fn render_pdf(path: &str, width: u32) -> Result<Vec<u8>, ApiError> {
    ensure_com();
    let hpath = HSTRING::from(path);
    let file: StorageFile = StorageFile::GetFileFromPathAsync(&hpath)
        .and_then(|op| op.join())
        .map_err(|e| ApiError::new("PDF", e.to_string()))?;
    let document: PdfDocument = PdfDocument::LoadFromFileAsync(&file)
        .and_then(|op| op.join())
        .map_err(|e| ApiError::new("PDF", e.to_string()))?;
    let page = document
        .GetPage(0)
        .map_err(|e| ApiError::new("PDF", e.to_string()))?;
    let stream =
        InMemoryRandomAccessStream::new().map_err(|e| ApiError::new("PDF", e.to_string()))?;
    let options = PdfPageRenderOptions::new().map_err(|e| ApiError::new("PDF", e.to_string()))?;
    options
        .SetDestinationWidth(width)
        .map_err(|e| ApiError::new("PDF", e.to_string()))?;
    // Width alone leaves a tall page free to render at any height; derive the height from
    // the page's own aspect ratio and cap it so the bitmap stays inside MAX_RENDER_BYTES.
    if let Ok(size) = page.Size() {
        if size.Width > 0.0 {
            let height = (f64::from(width) * f64::from(size.Height) / f64::from(size.Width)).ceil();
            let max_height = (MAX_RENDER_BYTES / (width as usize * 4)).max(1) as f64;
            let height = height.clamp(1.0, max_height).min(f64::from(u32::MAX));
            options
                .SetDestinationHeight(height as u32)
                .map_err(|e| ApiError::new("PDF", e.to_string()))?;
        }
    }
    page.RenderWithOptionsToStreamAsync(&stream, &options)
        .and_then(|op| op.join())
        .map_err(|e| ApiError::new("PDF", e.to_string()))?;

    let size = stream
        .Size()
        .map_err(|e| ApiError::new("PDF", e.to_string()))? as u32;
    if size == 0 {
        return Err(ApiError::new("PDF", "empty stream"));
    }
    stream
        .Seek(0)
        .map_err(|e| ApiError::new("PDF", e.to_string()))?;
    let reader =
        DataReader::CreateDataReader(&stream).map_err(|e| ApiError::new("PDF", e.to_string()))?;
    reader
        .LoadAsync(size)
        .and_then(|op| op.join())
        .map_err(|e| ApiError::new("PDF", e.to_string()))?;
    let mut bytes = vec![0u8; size as usize];
    reader
        .ReadBytes(&mut bytes)
        .map_err(|e| ApiError::new("PDF", e.to_string()))?;
    let _ = page.Close();
    Ok(bytes)
}

fn version_rows(path: &str) -> Vec<MetaRow> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(wide.as_ptr()), None);
        if size == 0 {
            return Vec::new();
        }
        let mut block = vec![0u8; size as usize];
        if GetFileVersionInfoW(
            PCWSTR(wide.as_ptr()),
            Some(0),
            size,
            block.as_mut_ptr().cast(),
        )
        .is_err()
        {
            return Vec::new();
        }
        let mut translations = std::ptr::null_mut();
        let mut length = 0u32;
        let ok = VerQueryValueW(
            block.as_ptr().cast(),
            w!("\\VarFileInfo\\Translation"),
            &mut translations,
            &mut length,
        );
        if !ok.as_bool() || translations.is_null() || length < 4 {
            return Vec::new();
        }
        let lang_cp = *(translations as *const u32);
        let mut rows = Vec::new();
        for (label, key) in [
            ("Product", "ProductName"),
            ("Description", "FileDescription"),
            ("Version", "FileVersion"),
            ("Company", "CompanyName"),
        ] {
            let sub = format!(
                "\\StringFileInfo\\{:04x}{:04x}\\{}",
                lang_cp & 0xffff,
                lang_cp >> 16,
                key
            );
            let sub_wide: Vec<u16> = sub.encode_utf16().chain(std::iter::once(0)).collect();
            let mut value_ptr = std::ptr::null_mut();
            let mut value_len = 0u32;
            let found = VerQueryValueW(
                block.as_ptr().cast(),
                PCWSTR(sub_wide.as_ptr()),
                &mut value_ptr,
                &mut value_len,
            );
            if !found.as_bool() || value_ptr.is_null() || value_len == 0 {
                continue;
            }
            let slice = std::slice::from_raw_parts(value_ptr as *const u16, value_len as usize);
            let text = String::from_utf16_lossy(slice)
                .trim_end_matches('\0')
                .to_string();
            if !text.is_empty() {
                rows.push(MetaRow {
                    label: label.to_string(),
                    value: text,
                });
            }
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ads_and_device_paths_are_rejected() {
        assert!(!has_ads_or_device(r"C:\data\file.txt"));
        assert!(has_ads_or_device(r"C:\data\file.txt:stream"));
        assert!(has_ads_or_device(r"C:\data\file.txt::$DATA"));
        assert!(has_ads_or_device(r"C:\data\CON.txt"));
        assert!(has_ads_or_device(r"C:\data\com1"));
        assert!(has_ads_or_device("C:\\data\\trailing."));
        assert!(has_ads_or_device("C:\\data\\trailing "));
        assert!(!has_ads_or_device(r"C:\data\console.txt"));
    }

    #[test]
    fn split_command_keeps_quoted_program_and_arguments() {
        assert_eq!(
            split_command(r#""C:\Program Files\Microsoft Office\WINWORD.EXE" /q "{path}""#),
            vec![r"C:\Program Files\Microsoft Office\WINWORD.EXE", "/q", "{path}"]
        );
        assert_eq!(split_command("  notepad   "), vec!["notepad"]);
        assert!(split_command("   ").is_empty());
    }

    #[test]
    fn command_label_uses_the_program_stem() {
        assert_eq!(command_label(r#""C:\Tools\WINWORD.EXE" "{path}""#), "WINWORD");
        assert_eq!(command_label("notepad"), "notepad");
    }

    #[test]
    fn sniff_rejects_binary_and_accepts_bom() {
        assert!(looks_like_text(b"fn main() {}\n"));
        assert!(!looks_like_text(&[0x4D, 0x5A, 0x00, 0x00]));
        assert!(looks_like_text(&[0xFF, 0xFE, b'a', 0x00]));
        assert!(!looks_like_text(b"\x89PNG\r\n\x1a\n\x00\x00"));
    }

    #[test]
    fn unlisted_text_previews_and_binary_falls_back_to_the_external_rule() {
        let dir = std::env::temp_dir();
        let lua = dir.join("flowdir_preview_probe.lua");
        std::fs::write(&lua, "print('hi')\n").unwrap();
        let text = info(
            lua.to_string_lossy().into_owned(),
            320,
            0xffffff,
            &[],
            Limits::default(),
            false,
        )
        .unwrap();
        assert_eq!(text.kind, "text");
        assert_eq!(text.text.as_deref(), Some("print('hi')\n"));

        let docx = dir.join("flowdir_preview_probe.docx");
        std::fs::write(&docx, [0x50, 0x4B, 0x03, 0x04, 0x00, 0x00]).unwrap();
        let rule = ExternalPreview {
            ext: "docx".into(),
            command: r#""C:\Tools\WINWORD.EXE" "{path}""#.into(),
        };
        let rule_info = info(
            docx.to_string_lossy().into_owned(),
            320,
            0xffffff,
            std::slice::from_ref(&rule),
            Limits::default(),
            false,
        )
        .unwrap();
        assert_eq!(rule_info.kind, "unsupported");
        assert_eq!(rule_info.external.as_deref(), Some("WINWORD"));

        let _ = std::fs::remove_file(&lua);
        let _ = std::fs::remove_file(&docx);
    }

    #[test]
    fn limits_cap_width_and_clamp_from_settings() {
        let limits = Limits::default();
        assert_eq!(limits.width(320), 320);
        assert_eq!(limits.width(0), 64);
        assert_eq!(limits.width(100_000), 4096);
        // A hand-edited value outside the documented range must not disable the cap.
        let settings = crate::settings::Settings {
            preview_max_dimension: 100_000,
            preview_max_file_mb: 0,
            preview_text_kb: 0,
            ..Default::default()
        }
        .normalize();
        let limits = Limits::from_settings(&settings);
        assert!(limits.max_dimension <= 16384);
        assert!(limits.max_bytes >= 1024 * 1024);
        assert!(limits.text_bytes >= 1024);
    }

    #[test]
    fn a_file_that_is_too_large_is_reported_without_reading_it() {
        let dir = std::env::temp_dir();
        let path = dir.join("flowdir_preview_oversize.txt");
        std::fs::write(&path, b"hello").unwrap();
        let limits = Limits {
            max_bytes: 1,
            ..Limits::default()
        };
        let info = info(
            path.to_string_lossy().into_owned(),
            320,
            0xffffff,
            &[],
            limits,
            false,
        )
        .unwrap();
        assert_eq!(info.kind, "unsupported");
        assert_eq!(info.reason.as_deref(), Some("too-large"));
        let _ = std::fs::remove_file(&path);
    }
}
