//! Path extraction shared by the OLE drop target (`dragdrop`) and the clipboard reader
//! (`clipboard`).
//!
//! Neither medium is obliged to publish `CF_HDROP`. VS Code publishes a drag as
//! `text/plain` only, and a clipboard copy made in a Chromium or Java application usually
//! carries text (or the pre-`CF_HDROP` `FileNameW` format) without a file list. Both
//! channels therefore need the same "is this text actually a list of files?" rules, and
//! keeping one implementation is what stops the two from disagreeing again.

use std::path::PathBuf;

use percent_encoding::percent_decode_str;
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

/// Text larger than this cannot be a path list and must not be scanned entry by entry.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

/// Copies the UTF-16 text held by an `HGLOBAL`, stopping at the first NUL.
///
/// The scan must stop at the allocation and at [`MAX_TEXT_BYTES`]: a hostile source can
/// hand over an unterminated block, and reading past it would be an out-of-bounds read. An
/// unterminated block is treated as malformed rather than truncated to the cap.
pub fn text_from_hglobal(hglobal: HGLOBAL) -> Option<String> {
    // SAFETY: the caller holds the medium that owns `hglobal` for this whole call.
    let units = unsafe { Units::new(hglobal)? };
    let capacity = units.capacity;
    let ptr = units.ptr;
    let mut len = 0usize;
    while len < capacity && unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    if len == capacity {
        return None;
    }
    Some(String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(ptr, len)
    }))
}

/// Copies a NUL-separated UTF-16 list (as used by `FileNameW`); the list ends at the first
/// empty entry or at the end of the block.
pub fn list_from_hglobal(hglobal: HGLOBAL) -> Vec<String> {
    let mut out = Vec::new();
    // SAFETY: the caller holds the medium that owns `hglobal` for this whole call.
    let Some(units) = (unsafe { Units::new(hglobal) }) else {
        return out;
    };
    let capacity = units.capacity;
    let ptr = units.ptr;
    let mut start = 0usize;
    while start < capacity {
        let mut end = start;
        while end < capacity && unsafe { *ptr.add(end) } != 0 {
            end += 1;
        }
        if end == start {
            break;
        }
        out.push(String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(ptr.add(start), end - start)
        }));
        start = end + 1;
    }
    out
}

/// A locked view over the block's readable range, capped by [`MAX_TEXT_BYTES`]. Dropping it
/// unlocks the block, so a reader cannot leave the medium locked on an early return.
struct Units {
    ptr: *const u16,
    capacity: usize,
    lock: HGLOBAL,
}

impl Units {
    /// # Safety
    /// `hglobal` must still be owned by a live medium for the lifetime of the returned
    /// value; the block is locked here and unlocked on drop.
    unsafe fn new(hglobal: HGLOBAL) -> Option<Self> {
        let size = unsafe { GlobalSize(hglobal) };
        if size < 2 {
            return None;
        }
        // One extra unit lets a fully terminated block be told apart from one cut off at
        // the cap, which is read as malformed instead of silently truncated.
        let capacity = (size / 2).min(MAX_TEXT_BYTES / 2 + 1);
        let ptr = unsafe { GlobalLock(hglobal) } as *const u16;
        if ptr.is_null() {
            return None;
        }
        Some(Self {
            ptr,
            capacity,
            lock: hglobal,
        })
    }
}

impl Drop for Units {
    fn drop(&mut self) {
        unsafe {
            let _ = GlobalUnlock(self.lock);
        }
    }
}

/// Strips one matching pair of surrounding quotes, which Explorer adds when copying a path
/// as text and which a plain `PathBuf` would keep as part of the file name.
fn unquote(line: &str) -> &str {
    let bytes = line.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        if (first == b'"' || first == b'\'') && bytes[bytes.len() - 1] == first {
            return &line[1..line.len() - 1];
        }
    }
    line
}

/// `D:\x` / `/D:/x` / `D:/x` → a local path; anything without a drive letter is not a local
/// Windows path and has no filesystem form.
fn drive_path(path: &str) -> Option<PathBuf> {
    let bytes = path.as_bytes();
    let has_drive = |offset: usize| {
        bytes.len() >= offset + 2
            && bytes[offset].is_ascii_alphabetic()
            && bytes[offset + 1] == b':'
    };
    let rest = if bytes.first() == Some(&b'/') && has_drive(1) {
        &path[1..]
    } else if has_drive(0) {
        path
    } else {
        return None;
    };
    let normalized = PathBuf::from(rest.replace('/', "\\"));
    normalized.is_absolute().then_some(normalized)
}

/// Turns a `file:` URI into a local path.
///
/// Every spelling that shows up in practice is understood:
/// `file:///D:/x` (Windows shell, `File::to_uri` in most tools), `file:/D:/x` (Java's
/// `File::toURI` emits a single slash), `file://localhost/D:/x`, the drive-as-authority
/// `file://D:/x`, and `file://server/share/x` as a UNC path. The URI is percent-decoded
/// before parsing, because a name containing `#`, `?` or a space arrives encoded.
pub fn file_uri_path(line: &str) -> Option<PathBuf> {
    let scheme = line.get(..5)?;
    if !scheme.eq_ignore_ascii_case("file:") {
        return None;
    }
    let decoded = percent_decode_str(&line[5..]).decode_utf8().ok()?;
    let decoded = decoded.as_ref();
    // Only the `//` form carries an authority; `file:/D:/x` has none.
    let (authority, path) = match decoded.strip_prefix("//") {
        Some(after) => match after.find('/') {
            Some(index) => (&after[..index], &after[index..]),
            None => (after, ""),
        },
        None => ("", decoded),
    };
    if authority.len() == 2 && authority.ends_with(':') {
        // `file://D:/x` — the drive letter was parsed as the authority.
        return drive_path(&format!("/{authority}{path}"));
    }
    if authority.is_empty() || authority.eq_ignore_ascii_case("localhost") {
        return drive_path(path);
    }
    // A named host is a UNC share.
    let share = PathBuf::from(format!(r"\\{authority}{}", path.replace('/', "\\")));
    share.is_absolute().then_some(share)
}

/// Reads a text payload as a list of local paths.
///
/// Editors such as VS Code send one absolute path per line. The whole payload is rejected
/// unless every non-empty line names an existing absolute path, so a dragged code snippet
/// that happens to contain a path is not turned into a file transfer.
pub fn parse_text_paths(text: &str) -> Vec<PathBuf> {
    if text.len() > MAX_TEXT_BYTES {
        return Vec::new();
    }
    let mut paths = Vec::new();
    // Windows clipboards use CRLF, some sources use a bare CR, and `text/uri-list` uses LF.
    for raw in text.split(['\r', '\n']) {
        if raw.trim().is_empty() {
            continue;
        }
        let path = file_uri_path(raw).unwrap_or_else(|| PathBuf::from(unquote(raw)));
        if !path.is_absolute() || std::fs::symlink_metadata(&path).is_err() {
            return Vec::new();
        }
        paths.push(path);
    }
    paths
}

/// Reads a `text/uri-list` payload, dropping the `#` comment lines RFC 2483 allows.
pub fn parse_uri_list(text: &str) -> Vec<PathBuf> {
    let entries: Vec<&str> = text
        .split(['\r', '\n'])
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect();
    parse_text_paths(&entries.join("\r\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> (PathBuf, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("flowdir-pathlist-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("file.txt");
        std::fs::write(&file, b"x").unwrap();
        (dir, file)
    }

    #[test]
    fn text_path_list_accepts_existing_absolute_paths() {
        let (dir, file) = temp_file("accept");
        let text = format!(
            "{}\r\n{}\r\n",
            file.to_string_lossy(),
            dir.to_string_lossy()
        );
        let paths = parse_text_paths(&text);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], file);
        assert_eq!(paths[1], dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_path_list_accepts_cr_only_and_quoted_paths() {
        let (dir, file) = temp_file("cr");
        let text = format!("\"{}\"\r{}", file.to_string_lossy(), file.to_string_lossy());
        let paths = parse_text_paths(&text);
        assert_eq!(paths, vec![file.clone(), file]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_path_list_rejects_prose_and_relative_paths() {
        for text in ["hello world", "file.txt\r\n", "https://example.invalid/x"] {
            assert!(parse_text_paths(text).is_empty());
        }
    }

    #[test]
    fn text_path_list_rejects_mixed_valid_and_invalid_lines() {
        let (dir, file) = temp_file("mixed");
        let text = format!("{}\r\nnot a path\r\n", file.to_string_lossy());
        assert!(parse_text_paths(&text).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_uris_cover_single_slash_and_localhost_forms() {
        let (dir, file) = temp_file("uri");
        let slashed = file.to_string_lossy().replace('\\', "/");
        for uri in [
            format!("file:///{slashed}"),
            format!("file:/{slashed}"),
            format!("file://localhost/{slashed}"),
        ] {
            assert_eq!(parse_text_paths(&uri), vec![file.clone()], "{uri}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_uris_decode_percent_escapes_and_unc_hosts() {
        let (dir, _file) = temp_file("plain");
        // A space and a `#` must survive the round trip through URI encoding.
        let spaced = dir.join("a b#c.txt");
        std::fs::write(&spaced, b"x").unwrap();
        let uri = format!(
            "file:///{}",
            spaced
                .to_string_lossy()
                .replace('\\', "/")
                .replace(' ', "%20")
                .replace('#', "%23")
        );
        assert_eq!(parse_text_paths(&uri), vec![spaced]);

        let unc = file_uri_path(r"file://server/share/x.txt").unwrap();
        assert_eq!(unc, PathBuf::from(r"\\server\share\x.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_path_list_accepts_non_ascii_paths() {
        // A Chinese file name is the norm on this locale, and a `String` path on Windows
        // still has to survive `PathBuf::from`. If this fails, every IDEA/VS Code drag and
        // every text-only clipboard paste silently rejects the whole list.
        let (dir, _file) = temp_file("unicode");
        let named = dir.join("新建 文档.txt");
        std::fs::write(&named, b"x").unwrap();
        let text = named.to_string_lossy().to_string();
        assert_eq!(parse_text_paths(&text), vec![named.clone()]);
        let uri = format!(
            "file:///{}",
            named.to_string_lossy().replace('\\', "/")
        );
        assert_eq!(parse_text_paths(&uri), vec![named]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_uri_without_a_drive_is_rejected() {
        assert!(file_uri_path("file:///etc/passwd").is_none());
    }

    #[test]
    fn uri_list_skips_comments() {
        let (dir, file) = temp_file("urilist");
        let text = format!(
            "# copied from a file manager\r\nfile:///{}\r\n",
            file.to_string_lossy().replace('\\', "/")
        );
        assert_eq!(parse_uri_list(&text), vec![file]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_file_schemes_are_not_paths() {
        for line in ["webdav://nas/a", "shell:RecycleBinFolder", "http://x/y"] {
            assert!(file_uri_path(line).is_none(), "{line}");
        }
    }
}
