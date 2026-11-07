//! WebDAV protocol client built on WinHTTP.
//!
//! The app addresses a remote server with a virtual path, `webdav://<connection-id>/<path>`,
//! so the existing tab/history/workspace model keeps working unchanged. Passwords are read
//! from Windows Credential Manager (see [`crate::credentials`]) and only WinHTTP sees them;
//! nothing in this module logs or serializes them.
//!
//! Transport choices:
//! - WinHTTP instead of a bundled TLS stack: the project already ships no extra network
//!   or crypto dependencies, and WinHTTP supplies proxy configuration, Basic/Digest/NTLM
//!   authentication and certificate validation from the OS.
//! - `quick-xml` for the `multistatus` body. The parser is streaming and namespace-agnostic
//!   (`d:`, `D:`, `lp1:` prefixes all occur in the wild).

use std::ffi::c_void;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use percent_encoding::percent_decode_str;
use quick_xml::events::Event;
use quick_xml::Reader;
use windows::core::PCWSTR;
use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetCredentials, WinHttpSetOption, WinHttpSetTimeouts, WinHttpWriteData,
    WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, WINHTTP_AUTH_SCHEME_BASIC, WINHTTP_AUTH_SCHEME_DIGEST,
    WINHTTP_AUTH_SCHEME_NEGOTIATE, WINHTTP_AUTH_SCHEME_NTLM, WINHTTP_AUTH_TARGET_SERVER,
    WINHTTP_CREDS_AUTHSCHEME,
    WINHTTP_OPEN_REQUEST_FLAGS, WINHTTP_OPTION_REDIRECT_POLICY,
    WINHTTP_OPTION_REDIRECT_POLICY_ALWAYS, WINHTTP_OPTION_SECURITY_FLAGS,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WINHTTP_QUERY_WWW_AUTHENTICATE,
};

use crate::credentials;
use crate::error::ApiError;
use crate::fs::{Entry, ScanOutcome};
use crate::settings::{Settings, WebdavConnection};

pub const SCHEME: &str = "webdav://";
const MAX_ENTRIES: usize = 100_000;
/// A `multistatus` body is XML metadata, not file content; 32 MiB is far beyond a
/// 100k-entry folder and stops a hostile server from exhausting memory.
const MAX_XML_BYTES: usize = 32 * 1024 * 1024;
const CHUNK: usize = 64 * 1024;
const CONNECT_TIMEOUT_MS: i32 = 15_000;
/// Large uploads/downloads may idle while a NAS spins up a disk; keep this generous.
const TRANSFER_TIMEOUT_MS: i32 = 120_000;
const MAX_AUTH_ATTEMPTS: u32 = 4;
const ATTR_DIRECTORY: u32 = 0x10;

/// Separates concurrent downloads of the same remote file (preview + open, for example).
static TEMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Parsed `webdav://<connection>/<relative path>` address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DavPath {
    pub connection: String,
    /// Slash-separated path below the connection's base URL; empty at the root.
    pub path: String,
}

/// True for an address the backend must route to the WebDAV client.
pub fn is_webdav(path: &str) -> bool {
    // `get` rejects non-UTF-8-boundary slices, which a plain byte range would panic on
    // for input such as `新建文件夹`.
    path.get(..SCHEME.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(SCHEME))
}

/// Splits a virtual address. Malformed addresses return `None` and are treated as an
/// ordinary path by the caller, which then fails with the usual `NOT_FOUND`.
pub fn parse(raw: &str) -> Option<DavPath> {
    if !is_webdav(raw) {
        return None;
    }
    let rest = raw[SCHEME.len()..].trim_start_matches('/');
    let (connection, tail) = rest.split_once('/').unwrap_or((rest, ""));
    if connection.is_empty() {
        return None;
    }
    Some(DavPath {
        connection: connection.to_string(),
        path: normalize_rel(tail),
    })
}

/// Builds a virtual path below `connection`.
pub fn virtual_path(connection: &str, path: &str) -> String {
    if path.is_empty() {
        format!("{SCHEME}{connection}/")
    } else {
        format!("{SCHEME}{connection}/{}", path.trim_start_matches('/'))
    }
}

/// Virtual child address. Remote paths always use `/`, independent of the host OS.
pub fn virtual_join(dir: &str, name: &str) -> String {
    let base = dir.trim_end_matches('/');
    format!("{base}/{}", name.trim_start_matches('/'))
}

/// Parent address; the connection root climbs to the drive overview (`''`) like a share
/// root does, so Back/Up keep their meaning.
pub fn virtual_parent(path: &str) -> String {
    let Some(dav) = parse(path) else {
        return String::new();
    };
    let mut parts: Vec<&str> = dav.path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.pop().is_none() {
        return String::new();
    }
    virtual_path(&dav.connection, &parts.join("/"))
}

fn normalize_rel(raw: &str) -> String {
    // The virtual path stores names exactly as the listing decoded them, so percent
    // decoding here would turn a literal `%2F` in a file name into a path separator.
    let normalized = raw.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for part in normalized.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            parts.pop();
            continue;
        }
        parts.push(part);
    }
    parts.join("/")
}

fn find<'a>(settings: &'a Settings, id: &str) -> Result<&'a WebdavConnection, ApiError> {
    settings
        .webdav
        .iter()
        .find(|conn| conn.id.eq_ignore_ascii_case(id))
        .ok_or_else(|| ApiError::new("WEBDAV_CONN", id.to_string()))
}

/// One remote row plus the fields the file list needs.
#[derive(Clone, Debug)]
pub struct RemoteStat {
    pub is_dir: bool,
    pub size: u64,
    pub mtime: u64,
}

/// Reads a folder through the connection that owns `path`.
pub fn list(
    settings: &Settings,
    path: &str,
    _hidden: bool,
    cancel: &AtomicBool,
) -> Result<ScanOutcome, ApiError> {
    let dav = parse(path).ok_or_else(|| ApiError::new("WEBDAV_CONN", path.to_string()))?;
    let conn = find(settings, &dav.connection)?;
    let client = DavClient::new(conn, None)?;
    let (entries, truncated) = client.list_detailed(&dav.path, cancel)?;
    Ok(ScanOutcome { entries, truncated })
}

/// Immediate subdirectory names for the sidebar tree.
pub fn tree_dirs(
    settings: &Settings,
    path: &str,
    _hidden: bool,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<ScanOutcome, ApiError> {
    let mut outcome = list(settings, path, false, cancel)?;
    outcome.entries.retain(|entry| entry.is_dir());
    let truncated = outcome.entries.len() > limit;
    outcome.entries.truncate(limit);
    outcome.truncated = truncated;
    Ok(outcome)
}

/// `Some` when the path exists, `None` on 404.
pub fn stat(settings: &Settings, path: &str) -> Result<Option<RemoteStat>, ApiError> {
    let dav = parse(path).ok_or_else(|| ApiError::new("WEBDAV_CONN", path.to_string()))?;
    let conn = find(settings, &dav.connection)?;
    let client = DavClient::new(conn, None)?;
    client.stat(&dav.path)
}

/// Existence check used by rename/paste conflict handling.
pub fn exists(settings: &Settings, path: &str) -> bool {
    stat(settings, path).ok().flatten().is_some()
}

/// Free `name (n)` variants for remote paths; local paths are left untouched so the caller
/// can hand them to `naming::unique`.
pub fn unique_paths(settings: &Settings, paths: &[String], sources: &[String]) -> Vec<String> {
    let mut out = paths.to_vec();
    // Folder listings are fetched once per destination instead of once per candidate.
    let mut listings: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::collections::HashMap::new();
    let mut claimed = std::collections::HashSet::new();

    for (index, path) in paths.iter().enumerate() {
        if !is_webdav(path) {
            continue;
        }
        let dir = virtual_parent(path);
        if !listings.contains_key(&dir) {
            let mut names = std::collections::HashSet::new();
            if let (Some(dav), Some(parent)) = (parse(path), parse(&dir)) {
                if let Ok(conn) = find(settings, &dav.connection) {
                    if let Ok(client) = DavClient::new(conn, None) {
                        if let Ok(entries) = client.list(&parent.path, &AtomicBool::new(false)) {
                            for entry in entries {
                                names.insert(entry.name.to_lowercase());
                            }
                        }
                    }
                }
            }
            listings.insert(dir.clone(), names);
        }
        let taken = listings.get_mut(&dir).expect("listing inserted above");
        let name = base_name(path).to_string();
        let source = sources.get(index).map(String::as_str);
        let mut candidate = name.clone();
        let mut n = 0u32;
        loop {
            let key = candidate.to_lowercase();
            let same_source = source.is_some_and(|s| base_name(s).eq_ignore_ascii_case(&candidate));
            if same_source || (!taken.contains(&key) && !claimed.contains(&key)) {
                taken.insert(key.clone());
                claimed.insert(key);
                break;
            }
            n += 1;
            if n > 10_000 {
                break;
            }
            candidate = suffixed_name(&name, n);
        }
        out[index] = virtual_join(&dir, &candidate);
    }
    out
}

fn suffixed_name(name: &str, n: u32) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} ({n}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({n})"),
    }
}

fn base_name(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path)
}

/// Downloads a remote file into the local cache and returns its path. Used for preview,
/// "open" and shell hand-off; the stamp file lets repeated previews skip the transfer when
/// size and modification time are unchanged.
pub fn ensure_local(
    cache_root: &Path,
    settings: &Settings,
    path: &str,
    max_bytes: Option<u64>,
) -> Result<PathBuf, ApiError> {
    let dav = parse(path).ok_or_else(|| ApiError::new("WEBDAV_CONN", path.to_string()))?;
    let conn = find(settings, &dav.connection)?;
    let client = DavClient::new(conn, None)?;
    let meta = client
        .stat(&dav.path)?
        .ok_or_else(|| ApiError::new("NOT_FOUND", path.to_string()))?;
    if meta.is_dir {
        return Err(ApiError::new("IS_DIR", path.to_string()));
    }
    if max_bytes.is_some_and(|limit| meta.size > limit) {
        return Err(ApiError::new("FILE_TOO_LARGE", path.to_string()));
    }

    let folder = cache_root.join(safe_component(&dav.connection));
    std::fs::create_dir_all(&folder).map_err(|e| ApiError::from_io(path, &e))?;
    let hash = fnv_hex(&dav.path);
    let name = safe_component(base_name(&dav.path));
    let target = folder.join(format!("{hash}-{name}"));
    let stamp = folder.join(format!("{hash}.stamp"));
    let signature = format!("{}:{}", meta.size, meta.mtime);
    if target.is_file()
        && std::fs::read_to_string(&stamp)
            .map(|value| value == signature)
            .unwrap_or(false)
    {
        return Ok(target);
    }

    let temp = folder.join(format!(
        "{hash}.{}.{}.part",
        std::process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let cancel = AtomicBool::new(false);
    client.get_to_file(&dav.path, &temp, &cancel)?;
    // A failed download must not leave a partial file that a later preview mistakes for
    // a complete one; only a successful transfer reaches the rename.
    if target.exists() {
        let _ = std::fs::remove_file(&target);
    }
    std::fs::rename(&temp, &target).map_err(|e| ApiError::from_io(path, &e))?;
    let _ = std::fs::write(&stamp, signature);
    Ok(target)
}

/// Verifies a connection and its credentials; used by the settings page's "test" button.
pub fn test(conn: &WebdavConnection, password: Option<&str>) -> Result<(), ApiError> {
    let client = DavClient::new(conn, password)?;
    client.stat("")?;
    Ok(())
}

fn safe_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        }
    }
    // `CON`, `NUL`, … are reserved device names even with an extension; a cache basename
    // must never become one.
    const RESERVED: [&str; 22] = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
        "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    let lower = out.to_lowercase();
    if out.is_empty() || RESERVED.contains(&lower.as_str()) {
        out.push('_');
    }
    out.chars().take(120).collect()
}

fn fnv_hex(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

// ---------------------------------------------------------------------------------------
// Address and HTTP plumbing
// ---------------------------------------------------------------------------------------

/// Encoding set for a path segment: everything that would change the URL's structure is
/// escaped, while `:` and `@` stay readable (servers accept them unescaped).
const SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

fn encode_segment(segment: &str) -> String {
    percent_encoding::utf8_percent_encode(segment, SEGMENT).to_string()
}

/// Drops `user[:password]@` from the authority before a URL is echoed in an error. The
/// settings form has separate credential fields, but a pasted URL may still carry them
/// and a password must never reach a log or an IPC response.
fn redact_userinfo(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let rest = &url[scheme_end + 3..];
    let Some(at) = rest.find('@') else {
        return url.to_string();
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    if at < authority_end {
        format!("{}://{}", &url[..scheme_end], &rest[at + 1..])
    } else {
        url.to_string()
    }
}

#[derive(Clone)]
struct Endpoint {
    secure: bool,
    host: String,
    port: u16,
    /// Percent-encoded base path, always starting with `/`.
    base: String,
}

impl Endpoint {
    fn parse(conn: &WebdavConnection) -> Result<Self, ApiError> {
        let url = conn.url.trim();
        let shown = redact_userinfo(url);
        let lower = url.to_ascii_lowercase();
        let (secure, rest) = if let Some(rest) = lower
            .strip_prefix("https://")
            .map(|_| &url[8..])
        {
            (true, rest)
        } else if let Some(rest) = lower.strip_prefix("http://").map(|_| &url[7..]) {
            (false, rest)
        } else {
            return Err(ApiError::new("WEBDAV_CONN", shown.clone()));
        };
        let (authority, raw_path) = match rest.split_once('/') {
            Some((authority, path)) => (authority, format!("/{path}")),
            None => (rest, String::new()),
        };
        // Userinfo is not supported; the username field is authoritative.
        let authority = authority.rsplit_once('@').map(|(_, host)| host).unwrap_or(authority);
        let default_port = if secure { 443 } else { 80 };
        let (host, port) = split_authority(authority, default_port)?;
        if host.is_empty()
            || host
                .chars()
                .any(|ch| ch.is_whitespace() || ch.is_control())
        {
            return Err(ApiError::new("WEBDAV_CONN", shown));
        }

        let mut base = String::new();
        for part in decoded_segments(&raw_path) {
            push_segment(&mut base, &part);
        }
        // The settings form may still hold an unsaved draft, so normalize here as well
        // instead of trusting `Settings::normalize` to have run.
        let mut root_parts: Vec<&str> = Vec::new();
        let root = conn.root.replace('\\', "/");
        for part in root.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    root_parts.pop();
                }
                other => root_parts.push(other),
            }
        }
        for part in root_parts {
            push_segment(&mut base, part);
        }
        if base.is_empty() {
            base.push('/');
        }
        Ok(Self {
            secure,
            host,
            port,
            base,
        })
    }

    fn origin(&self) -> String {
        let scheme = if self.secure { "https" } else { "http" };
        let default_port = if self.secure { 443 } else { 80 };
        if self.port == default_port {
            format!("{scheme}://{}", self.host)
        } else {
            format!("{scheme}://{}:{}", self.host, self.port)
        }
    }

    /// Percent-encoded request target for a relative path.
    fn object(&self, rel: &str) -> String {
        let mut path = self.base.clone();
        for part in rel.split('/').filter(|part| !part.is_empty()) {
            if !path.ends_with('/') {
                path.push('/');
            }
            path.push_str(&encode_segment(part));
        }
        // Collection targets conventionally end in `/`; only the root needs it guaranteed.
        if rel.is_empty() && !path.ends_with('/') {
            path.push('/');
        }
        path
    }

    fn url(&self, rel: &str) -> String {
        format!("{}{}", self.origin(), self.object(rel))
    }
}

fn split_authority(authority: &str, default_port: u16) -> Result<(String, u16), ApiError> {
    if let Some(rest) = authority.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return Err(ApiError::new("WEBDAV_CONN", authority.to_string()));
        };
        let host = format!("[{}]", &rest[..end]);
        let port = rest[end + 1..]
            .strip_prefix(':')
            .and_then(|value| value.parse().ok())
            .unwrap_or(default_port);
        return Ok((host, port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => Ok((
            host.to_string(),
            port.parse().unwrap_or(default_port),
        )),
        // A bare IPv6 literal has several colons and no port.
        _ => Ok((authority.to_string(), default_port)),
    }
}

fn decoded_segments(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in percent_decode_str(raw).decode_utf8_lossy().split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            out.pop();
            continue;
        }
        out.push(part.to_string());
    }
    out
}

fn push_segment(path: &mut String, segment: &str) {
    if !path.ends_with('/') {
        path.push('/');
    }
    path.push_str(&encode_segment(segment));
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

// WinHTTP handles are plain `HANDLE` values in this crate version, matching `update.rs`.
struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        // Closing every request/connection/session here is what keeps a long session from
        // leaking handles; the return value is irrelevant at this point.
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

/// The WinHTTP session and connection for one operation. Requests are short-lived handles;
/// the session is reused so parallel files do not each pay a TCP/TLS handshake.
pub(crate) struct DavClient {
    connection: WebdavConnection,
    password: Option<String>,
    endpoint: Endpoint,
    /// Held only for RAII: exceeding WinHTTP's per-session connection limit is the reason
    /// requests share one session instead of opening their own.
    _session: Handle,
    connect: Handle,
    /// Scheme learned from the first 401, reused for pre-authenticating large uploads.
    auth: Mutex<Option<WINHTTP_CREDS_AUTHSCHEME>>,
}

impl DavClient {
    pub(crate) fn new(
        connection: &WebdavConnection,
        override_password: Option<&str>,
    ) -> Result<Self, ApiError> {
        let password = match override_password {
            Some(value) => Some(value.to_string()),
            None => credentials::load(&connection.id),
        };
        let endpoint = Endpoint::parse(connection)?;
        let agent = wide("FlowDir WebDAV");
        let host = wide(&endpoint.host);
        unsafe {
            let session = WinHttpOpen(
                PCWSTR(agent.as_ptr()),
                WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            );
            // Handle-returning APIs report failure with NULL, not a Win32 error code.
            if session.is_null() {
                return Err(ApiError::new("WEBDAV_NETWORK", "WinHttpOpen failed"));
            }
            let session = Handle(session);
            let _ = WinHttpSetTimeouts(
                session.0,
                CONNECT_TIMEOUT_MS,
                CONNECT_TIMEOUT_MS,
                TRANSFER_TIMEOUT_MS,
                TRANSFER_TIMEOUT_MS,
            );
            let connect = WinHttpConnect(session.0, PCWSTR(host.as_ptr()), endpoint.port, 0);
            if connect.is_null() {
                return Err(ApiError::new("WEBDAV_NETWORK", "WinHttpConnect failed"));
            }
            Ok(Self {
                connection: connection.clone(),
                password,
                endpoint,
                _session: session,
                connect: Handle(connect),
                auth: Mutex::new(None),
            })
        }
    }

    fn user(&self) -> &str {
        self.connection.username.trim()
    }

    fn has_credentials(&self) -> bool {
        !self.user().is_empty() || self.password.as_deref().is_some_and(|p| !p.is_empty())
    }

    fn rel_url(&self, rel: &str) -> String {
        self.endpoint.url(rel)
    }

    /// One request/response cycle with the authentication handshake. `body` is sent with
    /// `WinHttpSendRequest`; uploads use [`Self::upload_file`] so the body can be streamed.
    fn request(
        &self,
        method: &str,
        rel: &str,
        headers: &str,
        body: Option<&[u8]>,
    ) -> Result<Response, ApiError> {
        let mut auth_attempted = false;
        for _ in 0..MAX_AUTH_ATTEMPTS {
            let handle = self.open(method, rel, headers, body, None)?;
            unsafe {
                WinHttpReceiveResponse(handle.0, std::ptr::null_mut())
                    .map_err(|e| ApiError::new("WEBDAV_NETWORK", e.to_string()))?;
            }
            let status = status_code(&handle)?;
            // One 401 is the server naming a scheme; the retry carries credentials. A second
            // 401 means those credentials were refused, so the response goes back as-is and
            // `status_error` reports `WEBDAV_AUTH`.
            if status == 401 && !auth_attempted && self.apply_credentials(&handle)? {
                auth_attempted = true;
                continue;
            }
            return Ok(Response { handle, status });
        }
        Err(ApiError::new(
            "WEBDAV_AUTH",
            format!("{method} {}: 认证失败", self.rel_url(rel)),
        ))
    }

    fn open(
        &self,
        method: &str,
        rel: &str,
        headers: &str,
        body: Option<&[u8]>,
        total: Option<u32>,
    ) -> Result<Handle, ApiError> {
        let verb = wide(method);
        let target = wide(&self.endpoint.object(rel));
        // Windows generates `WINHTTP_OPEN_REQUEST_FLAGS` as a newtype; build it explicitly so
        // the flag does not depend on the constant's generated shape.
        let flags = WINHTTP_OPEN_REQUEST_FLAGS(if self.endpoint.secure {
            0x0080_0000 // WINHTTP_FLAG_SECURE
        } else {
            0
        });
        let handle = unsafe {
            WinHttpOpenRequest(
                self.connect.0,
                PCWSTR(verb.as_ptr()),
                PCWSTR(target.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                flags,
            )
        };
        if handle.is_null() {
            return Err(ApiError::new("WEBDAV_NETWORK", "WinHttpOpenRequest failed"));
        }
        let handle = Handle(handle);
        self.apply_transport_options(&handle);
        self.preauth(&handle, headers)?;

        // WinHTTP takes the headers as a UTF-16 slice without its terminating NUL, and the
        // body as a byte slice rather than a pointer plus length.
        let header_wide = wide(headers);
        let header_slice = if headers.is_empty() {
            None
        } else {
            Some(&header_wide[..header_wide.len() - 1])
        };
        let optional = body.filter(|bytes| !bytes.is_empty());
        let optional_ptr = optional.map(|bytes| bytes.as_ptr() as *const c_void);
        let optional_len = optional.map(|bytes| bytes.len() as u32).unwrap_or(0);
        // `total` is set for streamed uploads; `WinHttpSendRequest` then leaves the request
        // open for `WinHttpWriteData`. A plain body supplies its own length.
        let total_len = total.unwrap_or(optional_len);
        unsafe {
            WinHttpSendRequest(
                handle.0,
                header_slice,
                optional_ptr,
                optional_len,
                total_len,
                0,
            )
            .map_err(|e| ApiError::new("WEBDAV_NETWORK", e.to_string()))?;
        }
        Ok(handle)
    }

    fn apply_transport_options(&self, handle: &Handle) {
        if self.connection.insecure_tls {
            // 0x0100 IGNORE_UNKNOWN_CA | 0x1000 IGNORE_CERT_CN_INVALID | 0x2000 IGNORE_CERT_DATE_INVALID
            let flags: u32 = 0x0100 | 0x1000 | 0x2000;
            let bytes: [u8; 4] = flags.to_le_bytes();
            unsafe {
                let _ = WinHttpSetOption(
                    Some(handle.0 as *const c_void),
                    WINHTTP_OPTION_SECURITY_FLAGS,
                    Some(&bytes[..]),
                );
            }
        }
        // The default policy only follows redirects for GET/HEAD; PROPFIND and PUT are
        // commonly redirected to the canonical `https://` or trailing-slash URL.
        let policy: u32 = WINHTTP_OPTION_REDIRECT_POLICY_ALWAYS;
        let policy_bytes: [u8; 4] = policy.to_le_bytes();
        unsafe {
            let _ = WinHttpSetOption(
                Some(handle.0 as *const c_void),
                WINHTTP_OPTION_REDIRECT_POLICY,
                Some(&policy_bytes[..]),
            );
        }
    }

    /// Adds Basic credentials up front (the common NAS setup) or applies a previously
    /// learned scheme so a large upload is not rejected after its body was sent.
    fn preauth(&self, handle: &Handle, headers: &str) -> Result<(), ApiError> {
        if headers.contains("Authorization:") || !self.has_credentials() {
            return Ok(());
        }
        let scheme = *self.auth.lock().unwrap();
        if let Some(scheme) = scheme {
            let user = wide(self.user());
            let pass = wide(self.password.as_deref().unwrap_or(""));
            unsafe {
                WinHttpSetCredentials(
                    handle.0,
                    WINHTTP_AUTH_TARGET_SERVER,
                    scheme.0,
                    PCWSTR(user.as_ptr()),
                    PCWSTR(pass.as_ptr()),
                    std::ptr::null_mut(),
                )
                .map_err(|e| ApiError::new("WEBDAV_AUTH", e.to_string()))?;
            }
        }
        Ok(())
    }

    /// Answers a 401 using the challenge header. Returns false when no supported scheme or
    /// no credentials are available, which turns into `WEBDAV_AUTH` at the caller.
    fn apply_credentials(&self, handle: &Handle) -> Result<bool, ApiError> {
        if !self.has_credentials() {
            return Ok(false);
        }
        let challenge = response_header(handle, WINHTTP_QUERY_WWW_AUTHENTICATE).unwrap_or_default();
        let Some(scheme) = pick_scheme(&challenge) else {
            return Ok(false);
        };
        *self.auth.lock().unwrap() = Some(scheme);
        let user = wide(self.user());
        let pass = wide(self.password.as_deref().unwrap_or(""));
        unsafe {
            WinHttpSetCredentials(
                handle.0,
                WINHTTP_AUTH_TARGET_SERVER,
                scheme.0,
                PCWSTR(user.as_ptr()),
                PCWSTR(pass.as_ptr()),
                std::ptr::null_mut(),
            )
            .map_err(|e| ApiError::new("WEBDAV_AUTH", e.to_string()))?;
        }
        Ok(true)
    }

    /// Uploads `local` to `rel`, streaming in chunks. Authentication is negotiated by
    /// rewinding and replaying when a 401 arrives; the body is only sent once credentials
    /// for the first attempt are known to be accepted.
    pub(crate) fn upload_file(
        &self,
        rel: &str,
        local: &Path,
        cancel: &AtomicBool,
    ) -> Result<(), ApiError> {
        let size = std::fs::metadata(local)
            .map_err(|e| ApiError::from_io(&local.to_string_lossy(), &e))?
            .len();
        if size > u64::from(u32::MAX) {
            return Err(ApiError::new(
                "FILE_TOO_LARGE",
                format!("{}: {size} bytes", local.display()),
            ));
        }
        let total = size as u32;
        let mut auth_attempted = false;
        for _ in 0..MAX_AUTH_ATTEMPTS {
            if cancel.load(Ordering::Relaxed) {
                return Err(ApiError::new("CANCELED", "upload canceled"));
            }
            let headers = "Content-Type: application/octet-stream\r\n";
            let handle = self.open("PUT", rel, headers, None, Some(total))?;
            let mut file = std::fs::File::open(local)
                .map_err(|e| ApiError::from_io(&local.to_string_lossy(), &e))?;
            let mut buffer = vec![0u8; CHUNK];
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(ApiError::new("CANCELED", "upload canceled"));
                }
                let read = file
                    .read(&mut buffer)
                    .map_err(|e| ApiError::from_io(&local.to_string_lossy(), &e))?;
                if read == 0 {
                    break;
                }
                let mut written = 0u32;
                unsafe {
                    WinHttpWriteData(
                        handle.0,
                        Some(buffer.as_ptr() as *const c_void),
                        read as u32,
                        &mut written,
                    )
                    .map_err(|e| ApiError::new("WEBDAV_NETWORK", e.to_string()))?;
                }
                if written as usize != read {
                    return Err(ApiError::new(
                        "WEBDAV_NETWORK",
                        format!("PUT {rel}: 仅写入 {written}/{read} 字节"),
                    ));
                }
            }
            unsafe {
                WinHttpReceiveResponse(handle.0, std::ptr::null_mut())
                    .map_err(|e| ApiError::new("WEBDAV_NETWORK", e.to_string()))?;
            }
            let status = status_code(&handle)?;
            if status == 401 && !auth_attempted {
                // Retrying after a refused password would upload the whole body again for
                // nothing, so only the first 401 (scheme unknown) is retried.
                if self.apply_credentials(&handle)? {
                    auth_attempted = true;
                    continue;
                }
                return Err(status_error(status, "PUT", rel));
            }
            if (200..300).contains(&status) {
                return Ok(());
            }
            return Err(status_error(status, "PUT", rel));
        }
        Err(ApiError::new("WEBDAV_AUTH", format!("PUT {rel}: 认证失败")))
    }

    /// Streams a remote file to `target`; the caller renames a complete temporary file
    /// into place so an aborted transfer never leaves a truncated cache entry.
    pub(crate) fn get_to_file(
        &self,
        rel: &str,
        target: &Path,
        cancel: &AtomicBool,
    ) -> Result<u64, ApiError> {
        let response = self.request("GET", rel, "", None)?;
        if !(200..300).contains(&response.status) {
            return Err(status_error(response.status, "GET", rel));
        }
        let file = std::fs::File::create(target)
            .map_err(|e| ApiError::from_io(&target.to_string_lossy(), &e))?;
        let mut writer = std::io::BufWriter::new(file);
        let written = response.copy_to(&mut writer, cancel)?;
        writer
            .flush()
            .map_err(|e| ApiError::from_io(&target.to_string_lossy(), &e))?;
        Ok(written)
    }

    /// `PROPFIND` depth 1 without the request target itself.
    pub(crate) fn list(&self, rel: &str, cancel: &AtomicBool) -> Result<Vec<Entry>, ApiError> {
        self.list_detailed(rel, cancel).map(|(entries, _)| entries)
    }

    /// Like [`Self::list`], but reports whether `MAX_ENTRIES` cut the result. A caller
    /// that deletes the source after moving its children must not treat a cut listing as
    /// complete.
    pub(crate) fn list_detailed(
        &self,
        rel: &str,
        cancel: &AtomicBool,
    ) -> Result<(Vec<Entry>, bool), ApiError> {
        let body = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:getcontentlength/><d:getlastmodified/><d:etag/></d:prop></d:propfind>"#;
        let response = self.request(
            "PROPFIND",
            rel,
            "Depth: 1\r\nContent-Type: application/xml; charset=utf-8\r\n",
            Some(body.as_bytes()),
        )?;
        if response.status == 404 {
            return Err(ApiError::new("NOT_FOUND", self.rel_url(rel)));
        }
        if response.status != 207 && response.status != 200 {
            return Err(status_error(response.status, "PROPFIND", rel));
        }
        let bytes = response.read_all(MAX_XML_BYTES, cancel)?;
        let rows = parse_multistatus(&bytes)?;
        let self_path = normalize_url_path(&self.endpoint.object(rel));
        // A 207 body that marks the request target itself as 404 means the folder is gone;
        // reporting an empty listing would look like a successful, empty folder.
        if rows
            .iter()
            .any(|row| row.missing && normalize_url_path(&row.href_path) == self_path)
        {
            return Err(ApiError::new("NOT_FOUND", self.rel_url(rel)));
        }
        let mut entries = Vec::new();
        let mut truncated = false;
        for row in rows {
            if row.missing || normalize_url_path(&row.href_path) == self_path {
                continue;
            }
            let name = match row.display_name {
                Some(name) if !name.is_empty() => name,
                _ => name_from_href(&row.href_path),
            };
            if name.is_empty() {
                continue;
            }
            // Check before pushing: a folder with exactly MAX_ENTRIES children is
            // complete, and only an additional row proves the listing was cut.
            if entries.len() >= MAX_ENTRIES {
                truncated = true;
                break;
            }
            let attrs = if row.is_dir { ATTR_DIRECTORY } else { 0 };
            let id = fnv_u64(&format!("{}:{}:{}", row.href_path, row.size, row.mtime));
            entries.push(Entry::new(name, row.size, row.mtime, attrs, id));
        }
        Ok((entries, truncated))
    }

    /// `PROPFIND` depth 0. A 404 is a normal answer here ("does not exist"), not an error.
    pub(crate) fn stat(&self, rel: &str) -> Result<Option<RemoteStat>, ApiError> {
        let body = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:getcontentlength/><d:getlastmodified/><d:etag/></d:prop></d:propfind>"#;
        let response = self.request(
            "PROPFIND",
            rel,
            "Depth: 0\r\nContent-Type: application/xml; charset=utf-8\r\n",
            Some(body.as_bytes()),
        )?;
        if response.status == 404 {
            return Ok(None);
        }
        if response.status != 207 && response.status != 200 {
            return Err(status_error(response.status, "PROPFIND", rel));
        }
        let bytes = response.read_all(MAX_XML_BYTES, &AtomicBool::new(false))?;
        let rows = parse_multistatus(&bytes)?;
        Ok(rows
            .into_iter()
            .find(|row| !row.missing)
            .map(|row| RemoteStat {
                is_dir: row.is_dir,
                size: row.size,
                mtime: row.mtime,
            }))
    }

    pub(crate) fn mkdir(&self, rel: &str) -> Result<(), ApiError> {
        let response = self.request("MKCOL", rel, "", None)?;
        match response.status {
            200 | 201 | 204 => Ok(()),
            405 => Err(ApiError::new("EXISTS", self.rel_url(rel))),
            _ => Err(status_error(response.status, "MKCOL", rel)),
        }
    }

    pub(crate) fn delete(&self, rel: &str) -> Result<(), ApiError> {
        let response = self.request("DELETE", rel, "", None)?;
        match response.status {
            200 | 202 | 204 => Ok(()),
            404 => Err(ApiError::new("NOT_FOUND", self.rel_url(rel))),
            _ => Err(status_error(response.status, "DELETE", rel)),
        }
    }

    pub(crate) fn move_to(
        &self,
        from: &str,
        to: &str,
        overwrite: bool,
    ) -> Result<(), ApiError> {
        let headers = format!(
            "Destination: {}\r\nOverwrite: {}\r\n",
            self.rel_url(to),
            if overwrite { "T" } else { "F" }
        );
        self.transfer_verb("MOVE", from, &headers)
    }

    pub(crate) fn copy_to(
        &self,
        from: &str,
        to: &str,
        overwrite: bool,
    ) -> Result<(), ApiError> {
        let headers = format!(
            "Destination: {}\r\nOverwrite: {}\r\n",
            self.rel_url(to),
            if overwrite { "T" } else { "F" }
        );
        self.transfer_verb("COPY", from, &headers)
    }

    fn transfer_verb(&self, verb: &str, from: &str, headers: &str) -> Result<(), ApiError> {
        let response = self.request(verb, from, headers, None)?;
        match response.status {
            200 | 201 | 204 => Ok(()),
            404 => Err(ApiError::new("NOT_FOUND", self.rel_url(from))),
            412 => Err(ApiError::new("EXISTS", self.rel_url(from))),
            _ => Err(status_error(response.status, verb, from)),
        }
    }

    pub(crate) fn url_for(&self, rel: &str) -> String {
        self.rel_url(rel)
    }
}

fn fnv_u64(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

struct Response {
    handle: Handle,
    status: u32,
}

impl Response {
    fn read_all(&self, cap: usize, cancel: &AtomicBool) -> Result<Vec<u8>, ApiError> {
        let mut out = Vec::new();
        self.copy_into(&mut out, cancel, Some(cap))?;
        Ok(out)
    }

    fn copy_to<W: Write>(&self, writer: &mut W, cancel: &AtomicBool) -> Result<u64, ApiError> {
        self.copy_into(writer, cancel, None)
    }

    fn copy_into<W: Write>(
        &self,
        writer: &mut W,
        cancel: &AtomicBool,
        cap: Option<usize>,
    ) -> Result<u64, ApiError> {
        let mut total = 0u64;
        let mut buffer = vec![0u8; CHUNK];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(ApiError::new("CANCELED", "transfer canceled"));
            }
            let mut available = 0u32;
            unsafe {
                WinHttpQueryDataAvailable(self.handle.0, &mut available)
                    .map_err(|e| ApiError::new("WEBDAV_NETWORK", e.to_string()))?;
            }
            if available == 0 {
                break;
            }
            let want = available.min(buffer.len() as u32);
            let mut read = 0u32;
            unsafe {
                WinHttpReadData(
                    self.handle.0,
                    buffer.as_mut_ptr() as *mut c_void,
                    want,
                    &mut read,
                )
                .map_err(|e| ApiError::new("WEBDAV_NETWORK", e.to_string()))?;
            }
            if read == 0 {
                break;
            }
            if cap.is_some_and(|limit| total + u64::from(read) > limit as u64) {
                return Err(ApiError::new("FILE_TOO_LARGE", "response body"));
            }
            writer
                .write_all(&buffer[..read as usize])
                .map_err(|e| ApiError::new("IO", e.to_string()))?;
            total += u64::from(read);
        }
        Ok(total)
    }
}

fn status_code(handle: &Handle) -> Result<u32, ApiError> {
    let mut code = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    unsafe {
        WinHttpQueryHeaders(
            handle.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut code as *mut u32 as *mut c_void),
            &mut size,
            std::ptr::null_mut(),
        )
        .map_err(|e| ApiError::new("WEBDAV_NETWORK", e.to_string()))?;
    }
    Ok(code)
}

fn response_header(handle: &Handle, query: u32) -> Option<String> {
    let mut buffer = vec![0u16; 8192];
    let mut size = (buffer.len() * 2) as u32;
    let ok = unsafe {
        WinHttpQueryHeaders(
            handle.0,
            query,
            PCWSTR::null(),
            Some(buffer.as_mut_ptr() as *mut c_void),
            &mut size,
            std::ptr::null_mut(),
        )
        .is_ok()
    };
    if !ok {
        return None;
    }
    let chars = (size as usize / 2).min(buffer.len());
    let text = String::from_utf16_lossy(&buffer[..chars]);
    Some(text.trim_end_matches('\0').to_string())
}

fn pick_scheme(challenge: &str) -> Option<WINHTTP_CREDS_AUTHSCHEME> {
    let lower = challenge.to_ascii_lowercase();
    // Security order: Digest never puts the password on the wire; NTLM/Negotiate are
    // preferable to Basic when the server offers both.
    if lower.contains("digest") {
        // The metadata declares only this scheme as a bare `u32`; wrap it so the return
        // type stays consistent with the other constants.
        Some(WINHTTP_CREDS_AUTHSCHEME(WINHTTP_AUTH_SCHEME_DIGEST))
    } else if lower.contains("ntlm") {
        Some(WINHTTP_AUTH_SCHEME_NTLM)
    } else if lower.contains("negotiate") {
        Some(WINHTTP_AUTH_SCHEME_NEGOTIATE)
    } else if lower.contains("basic") {
        Some(WINHTTP_AUTH_SCHEME_BASIC)
    } else {
        None
    }
}

fn status_error(status: u32, verb: &str, rel: &str) -> ApiError {
    let code = match status {
        401 => "WEBDAV_AUTH",
        403 | 423 => "ACCESS_DENIED",
        404 => "NOT_FOUND",
        405 | 501 => "BAD_VERB",
        409 | 412 => "EXISTS",
        413 => "FILE_TOO_LARGE",
        507 => "DISK_FULL",
        400 => "INVALID_PATH",
        _ if status >= 500 => "WEBDAV_SERVER",
        _ => "WEBDAV_PROTOCOL",
    };
    ApiError::new(code, format!("{verb} {rel}: HTTP {status}"))
}

// ---------------------------------------------------------------------------------------
// PROPFIND parsing
// ---------------------------------------------------------------------------------------

struct DavRow {
    href_path: String,
    display_name: Option<String>,
    is_dir: bool,
    size: u64,
    mtime: u64,
    /// A `propstat`/`response` status of 404. Plenty of servers answer PROPFIND for a
    /// missing path with 207 plus a 404 status in the body instead of a bare 404.
    missing: bool,
    /// At least one `propstat` answered 2xx. Servers routinely split a resource into a
    /// successful `propstat` plus a 404 one for properties they do not implement
    /// (`getcontentlength`/`etag` on collections, for example); that is a normal listing,
    /// not a missing resource.
    ok: bool,
}

/// Parses a `D:multistatus` body. Namespace prefixes are ignored by comparing only the
/// local element name, which is what servers actually differ in.
fn parse_multistatus(bytes: &[u8]) -> Result<Vec<DavRow>, ApiError> {
    let text = String::from_utf8_lossy(bytes);
    let mut reader = Reader::from_str(&text);
    reader.config_mut().trim_text(true);
    let mut rows = Vec::new();
    let mut current: Option<DavRow> = None;
    let mut capture: Option<Field> = None;
    let mut in_resource_type = false;
    let mut buf = Vec::new();

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| ApiError::new("WEBDAV_XML", e.to_string()))?;
        match event {
            Event::Start(element) => {
                let name = element.local_name();
                // `Reader::from_str` yields names as `&str`; the `d:`/`D:` prefix is already
                // stripped by `local_name`.
                match name.as_ref() {
                    "response" => {
                        current = Some(DavRow {
                            href_path: String::new(),
                            display_name: None,
                            is_dir: false,
                            size: 0,
                            mtime: 0,
                            missing: false,
                            ok: false,
                        })
                    }
                    "href" => capture = Some(Field::Href),
                    "displayname" => capture = Some(Field::DisplayName),
                    "getcontentlength" => capture = Some(Field::Size),
                    "getlastmodified" => capture = Some(Field::Mtime),
                    "status" => capture = Some(Field::Status),
                    "resourcetype" => {
                        in_resource_type = true;
                        capture = None;
                    }
                    "collection" if in_resource_type => {
                        if let Some(row) = current.as_mut() {
                            row.is_dir = true;
                        }
                    }
                    _ => capture = None,
                }
            }
            Event::Empty(element) => {
                if element.local_name().as_ref() == "collection" {
                    if let Some(row) = current.as_mut() {
                        row.is_dir = true;
                    }
                }
            }
            Event::Text(value) => {
                // quick-xml 0.42 hands out escaped text; hrefs and display names must come
                // back as the literal characters the server meant.
                let raw = value.xml10_content();
                let text = quick_xml::escape::unescape(&raw)
                    .map(|text| text.into_owned())
                    .unwrap_or_else(|_| raw.into_owned());
                capture_text(capture, current.as_mut(), &text);
            }
            Event::CData(value) => {
                // CDATA is literal; only EOL normalisation applies.
                let text = value.xml10_content().into_owned();
                capture_text(capture, current.as_mut(), &text);
            }
            Event::End(element) => {
                let name = element.local_name();
                match name.as_ref() {
                    "response" => {
                        if let Some(mut row) = current.take() {
                            // A 404 `propstat` only means "this property is unavailable";
                            // the resource is missing only when no `propstat` succeeded.
                            row.missing = row.missing && !row.ok;
                            rows.push(row);
                        }
                        capture = None;
                        in_resource_type = false;
                    }
                    "resourcetype" => {
                        in_resource_type = false;
                        capture = None;
                    }
                    "href" | "displayname" | "getcontentlength" | "getlastmodified"
                    | "status" => capture = None,
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(rows)
}

#[derive(Clone, Copy)]
enum Field {
    Href,
    DisplayName,
    Size,
    Mtime,
    Status,
}

fn capture_text(field: Option<Field>, row: Option<&mut DavRow>, text: &str) {
    let (Some(field), Some(row)) = (field, row) else {
        return;
    };
    let text = text.trim();
    match field {
        Field::Href => row.href_path.push_str(text),
        // `displayname` is optional; a server may split it across several text nodes.
        Field::DisplayName => match &mut row.display_name {
            Some(name) => name.push_str(text),
            None => row.display_name = Some(text.to_string()),
        },
        Field::Size => row.size = text.parse().unwrap_or(0),
        Field::Mtime => row.mtime = parse_http_date(text),
        // `HTTP/1.1 200 OK` / `HTTP/1.1 404 Not Found`, one `propstat` at a time.
        Field::Status => match text.split_whitespace().nth(1) {
            Some(code) if code.starts_with('2') => row.ok = true,
            Some("404") => row.missing = true,
            _ => {}
        },
    }
}

/// Compares URL paths after decoding and dropping a trailing slash. Servers may return an
/// absolute URL or an absolute path; both are reduced to the path part.
fn normalize_url_path(raw: &str) -> String {
    // Some servers emit an absolute URL with an upper-case scheme; compare the prefix
    // case-insensitively while slicing the original so percent escapes stay intact.
    let lower = raw.to_ascii_lowercase();
    let path = if lower.starts_with("http://") {
        let rest = &raw[7..];
        rest.find('/').map(|index| &rest[index..]).unwrap_or("/")
    } else if lower.starts_with("https://") {
        let rest = &raw[8..];
        rest.find('/').map(|index| &rest[index..]).unwrap_or("/")
    } else {
        raw
    };
    let decoded = percent_decode_str(path).decode_utf8_lossy();
    decoded.trim_end_matches('/').to_string()
}

fn name_from_href(href: &str) -> String {
    let path = normalize_url_path(href);
    path.rsplit('/').find(|part| !part.is_empty()).unwrap_or("").to_string()
}

/// RFC 1123 (`Wed, 21 Oct 2015 07:28:00 GMT`) and ISO-8601 (`2015-10-21T07:28:00Z`).
/// Returns Unix milliseconds, or 0 when the value cannot be parsed.
fn parse_http_date(raw: &str) -> u64 {
    let text = raw.trim();
    // Normalise both accepted forms to `YYYY-MM-DD`, `HH:MM:SS`. The ISO form is detected
    // by its digit-dash prefix rather than by `split_once('T')`: RFC 1123 values end in
    // `GMT`, whose `T` would otherwise be taken as the date separator.
    let iso = text.len() >= 11 && text.as_bytes()[4] == b'-';
    let (date, time) = if iso {
        let Some((date, time)) = text.split_once('T') else {
            return 0;
        };
        (date.to_string(), time.trim_end_matches('Z').to_string())
    } else {
        // `Wed, 21 Oct 2015 07:28:00 GMT`
        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.len() < 5 {
            return 0;
        }
        let Ok(day) = parts[1].parse::<u32>() else {
            return 0;
        };
        let month = month_number(parts[2]);
        let Ok(year) = parts[3].parse::<i32>() else {
            return 0;
        };
        if month == 0 {
            return 0;
        }
        (
            format!("{year:04}-{month:02}-{day:02}"),
            parts[4].to_string(),
        )
    };
    let Some((y, rest)) = date.split_once('-') else {
        return 0;
    };
    let Some((m, d)) = rest.split_once('-') else {
        return 0;
    };
    let (Ok(year), Ok(month), Ok(day)) = (y.parse::<i32>(), m.parse::<u32>(), d.parse::<u32>()) else {
        return 0;
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return 0;
    }
    let mut clock = time.split(':');
    let hour = clock.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    let minute = clock.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    let second = clock
        .next()
        .and_then(|v| v.split('.').next())
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second;
    if seconds < 0 {
        0
    } else {
        seconds as u64 * 1000
    }
}

fn month_number(name: &str) -> u32 {
    match name.to_ascii_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => 0,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let m = month as i64;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe as i64 * 365 + yoe as i64 / 4 - yoe as i64 / 100 + doy;
    era as i64 * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_virtual_addresses() {
        assert_eq!(parse("webdav://nas/"), Some(DavPath { connection: "nas".into(), path: String::new() }));
        assert_eq!(
            parse("webdav://nas/a/b"),
            Some(DavPath { connection: "nas".into(), path: "a/b".into() })
        );
        assert_eq!(
            parse("webdav://nas/a/../b/./c"),
            Some(DavPath { connection: "nas".into(), path: "b/c".into() })
        );
        assert_eq!(parse("C:\\data"), None);
    }

    #[test]
    fn joins_and_parents() {
        assert_eq!(virtual_join("webdav://nas/", "x"), "webdav://nas/x");
        assert_eq!(virtual_join("webdav://nas/a", "x"), "webdav://nas/a/x");
        assert_eq!(virtual_parent("webdav://nas/a/x"), "webdav://nas/a");
        assert_eq!(virtual_parent("webdav://nas/a"), "webdav://nas/");
        assert_eq!(virtual_parent("webdav://nas/"), "");
    }

    #[test]
    fn parses_http_dates() {
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), 0);
        // 2015-10-21T07:28:00Z
        assert_eq!(parse_http_date("2015-10-21T07:28:00Z"), 1_445_412_480_000);
        assert_eq!(parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT"), 1_445_412_480_000);
    }

    #[test]
    fn extracts_names_from_hrefs() {
        assert_eq!(name_from_href("/dav/photos/a%20b.jpg"), "a b.jpg");
        assert_eq!(name_from_href("https://nas/dav/dir/"), "dir");
    }

    #[test]
    fn compares_hrefs_after_decoding() {
        // `Endpoint::object` percent-encodes the request target, while servers may answer
        // with raw UTF-8 or a differently escaped href. The self-filter has to compare
        // decoded paths, otherwise the listed folder shows up as one of its own children.
        assert_eq!(
            normalize_url_path("/dav/a%20b"),
            normalize_url_path("/dav/a b")
        );
        assert_eq!(
            normalize_url_path("https://nas/dav/%E4%B8%AD%E6%96%87/"),
            "/dav/中文"
        );
        assert_eq!(normalize_url_path("/dav/a%2Fb"), "/dav/a/b");
        assert_eq!(
            normalize_url_path("/webdav/"),
            normalize_url_path("/webdav")
        );
    }

    #[test]
    fn parses_propfind_multistatus() {
        let xml = br#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response><d:href>/dav/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
  <d:response><d:href>/dav/file.txt</d:href><d:propstat><d:prop><d:resourcetype/><d:getcontentlength>12</d:getcontentlength><d:getlastmodified>Wed, 21 Oct 2015 07:28:00 GMT</d:getlastmodified></d:prop></d:propstat></d:response>
</d:multistatus>"#;
        let rows = parse_multistatus(xml).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].is_dir);
        assert_eq!(rows[1].size, 12);
        assert_eq!(rows[1].mtime, 1_445_412_480_000);
    }

    #[test]
    fn marks_missing_resources() {
        let xml = br#"<d:multistatus xmlns:d="DAV:">
  <d:response><d:href>/dav/gone.txt</d:href><d:propstat><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response>
  <d:response><d:href>/dav/ok.txt</d:href><d:propstat><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
</d:multistatus>"#;
        let rows = parse_multistatus(xml).unwrap();
        assert!(rows[0].missing);
        assert!(!rows[1].missing);
    }

    #[test]
    fn a_failed_propstat_does_not_hide_a_listed_resource() {
        // What fnOS returns for a collection: the properties it supports under a 200
        // propstat, and `getcontentlength`/`etag` under a 404 one. Treating the 404 as
        // "resource missing" made every folder look like NOT_FOUND.
        let xml = br#"<D:multistatus xmlns:D="DAV:">
  <D:response><D:href>/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype><D:getlastmodified>Thu, 19 Dec 2024 09:32:53 GMT</D:getlastmodified></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
    <D:propstat><D:prop><D:getcontentlength></D:getcontentlength><D:etag></D:etag></D:prop><D:status>HTTP/1.1 404 Not Found</D:status></D:propstat>
  </D:response>
  <D:response><D:href>/webdav/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
</D:multistatus>"#;
        let rows = parse_multistatus(xml).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| !row.missing));
        assert!(rows[0].is_dir);
    }
}
