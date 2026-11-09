//! Archive entry names: raw-byte path splitting, legacy code-page decoding and the
//! Windows name rules every extracted path has to pass.
//!
//! Names are split on raw `/` bytes *before* decoding. GBK and Shift-JIS trail bytes can
//! be `0x5C` (`\`), so decoding first and splitting afterwards would let a crafted entry
//! smuggle a separator past the traversal check.

use std::sync::OnceLock;

use windows::Win32::Globalization::{GetACP, GetUserDefaultUILanguage, MultiByteToWideChar, MB_ERR_INVALID_CHARS};

use crate::error::ApiError;

/// IBM code page 437, the zip fallback for names without the UTF-8 flag.
const CP437: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', //
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', //
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', //
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐', //
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧', //
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀', //
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', //
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{a0}',
];

/// Windows device names are reserved with any extension (`CON.txt`, `NUL.tar.gz`).
const RESERVED: [&str; 24] = [
    "con", "prn", "aux", "nul", "conin$", "conout$", "com1", "com2", "com3", "com4", "com5",
    "com6", "com7", "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7",
    "lpt8", "lpt9",
];

/// Decodes a raw archive name into a relative `\`-separated path.
///
/// Both `/` and `\` split segments: Windows cannot represent a literal `\` in a file name,
/// and 7-Zip on Windows (plus a number of repacking tools) writes nested 7z/zip entries
/// with backslashes. Treating them as literal characters made every nested entry of such
/// an archive fail the safety check and disappear from the extraction.
///
/// `utf8_flag` is the zip bit 11 marker. When clear (and for tar), a name that is valid
/// UTF-8 is still taken as UTF-8 — writers that omit the flag are common and UTF-8 bytes
/// almost never form valid text in a legacy code page. Otherwise the system ANSI code page
/// is tried (what WinRAR/7-Zip do for names produced on a localized Windows) and CP437 is
/// the last resort the zip specification itself names.
pub fn decode(raw: &[u8], utf8_flag: bool) -> Result<String, ApiError> {
    decode_with(raw, utf8_flag, true)
}

/// RAR and CAB store path separators as `\`; every other backend in this crate uses `/`.
pub fn decode_rar(raw: &[u8], utf8_flag: bool) -> Result<String, ApiError> {
    decode_with(raw, utf8_flag, true)
}

fn decode_with(raw: &[u8], utf8_flag: bool, backslash: bool) -> Result<String, ApiError> {
    decode_parts(raw, utf8_flag, backslash, None)
}

/// `page` overrides the legacy code page; only tests use it.
fn decode_parts(
    raw: &[u8],
    utf8_flag: bool,
    backslash: bool,
    page: Option<u32>,
) -> Result<String, ApiError> {
    if raw.starts_with(b"/") || raw.starts_with(b"\\") {
        return Err(ApiError::new("UNSAFE_PATH", "absolute entry name"));
    }
    if raw.contains(&0) {
        return Err(ApiError::new("UNSAFE_PATH", "NUL in entry name"));
    }
    let mut segments = Vec::new();
    // `./name` is how most tar writers spell a member under the archive root; the `.` is a
    // path marker, not a file name, and rejecting it dropped every member of such archives.
    let mut saw_root_marker = false;
    // Only `/` may split the raw bytes: GBK and Shift-JIS use 0x5C (`\`) as a trail byte,
    // so a backslash becomes a separator only after the name is decoded.
    for part in raw.split(|byte| *byte == b'/') {
        if part.is_empty() {
            // `a//b` and a trailing `/` (directory marker) are harmless.
            continue;
        }
        let text = if utf8_flag {
            std::str::from_utf8(part)
                .map_err(|_| ApiError::new("INVALID_NAME", "entry name is not UTF-8"))?
                .to_string()
        } else if let Ok(text) = std::str::from_utf8(part) {
            text.to_string()
        } else {
            decode_legacy(part, page)
        };
        if backslash {
            for piece in text.split('\\') {
                if piece == "." {
                    saw_root_marker = true;
                } else if !piece.is_empty() {
                    segments.push(sanitize_segment(piece)?);
                }
            }
        } else if text == "." {
            saw_root_marker = true;
        } else {
            segments.push(sanitize_segment(&text)?);
        }
    }
    if segments.is_empty() {
        // A member that only names the root (`./`) carries no content. Reporting it as a
        // failure would make every ordinary tar archive look partly broken.
        if saw_root_marker {
            return Err(ApiError::new(ROOT_MARKER, "archive root marker"));
        }
        return Err(ApiError::new("INVALID_NAME", "empty entry name"));
    }
    Ok(segments.join("\\"))
}

/// Code for a member that names the archive root itself, not a file. Callers skip it
/// silently; it is not a broken entry.
pub const ROOT_MARKER: &str = "ROOT_MARKER";

/// Whether a decode error is the archive root marker rather than an unsafe name.
pub fn is_root_marker(err: &ApiError) -> bool {
    err.code == ROOT_MARKER
}

/// Legacy code page of the user's Windows language, then GBK, then CP437 — the page the
/// zip specification itself falls back to.
///
/// The second pass matters on CJK machines: zh-TW/ja/ko use 950/932/949, which reject
/// some GBK sequences, so 936 rescues those names. Single-byte pages (437, 1252) accept
/// most bytes, so there the host page wins — the same result as Explorer and WinRAR.
/// The "UTF-8 worldwide language support" beta reports 65001, under which every legacy
/// CJK name is invalid, hence the 0 page marker.
fn decode_legacy(bytes: &[u8], page: Option<u32>) -> String {
    if let Some(page) = page {
        return decode_bytes_page(bytes, page).unwrap_or_else(|| cp437_fallback(bytes));
    }
    for page in [language_codepage(), 936] {
        if let Some(text) = decode_bytes_page(bytes, page) {
            return text;
        }
    }
    cp437_fallback(bytes)
}

/// Last resort named by the zip specification: every byte has a glyph, so decoding never
/// fails. Kept separate so its table is testable without depending on the host page.
fn cp437_fallback(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| {
            if *byte < 0x80 {
                *byte as char
            } else {
                CP437[(*byte - 0x80) as usize]
            }
        })
        .collect()
}

fn decode_bytes_page(bytes: &[u8], page: u32) -> Option<String> {
    if page == 0 {
        return None;
    }
    let mut wide = vec![0u16; bytes.len() + 1];
    let len = unsafe { MultiByteToWideChar(page, MB_ERR_INVALID_CHARS, bytes, Some(&mut wide)) };
    (len > 0).then(|| String::from_utf16_lossy(&wide[..len as usize]))
}

/// ANSI page for the UI language, or the system page when it is not UTF-8.
fn language_codepage() -> u32 {
    static PAGE: OnceLock<u32> = OnceLock::new();
    *PAGE.get_or_init(|| {
        let language = unsafe { GetUserDefaultUILanguage() };
        match (language & 0x03ff, language >> 10) {
            // Traditional Chinese locales (TW/HK/MO) use 950, the rest of zh uses 936.
            (0x04, 1) => 950,
            (0x04, _) => 936,
            (0x11, _) => 932,
            (0x12, _) => 949,
            _ => {
                let ansi = unsafe { GetACP() };
                if ansi == 65001 {
                    0
                } else {
                    ansi
                }
            }
        }
    })
}

/// Applies the Windows file-name rules to one component. Reserved names and trailing
/// dots/spaces are repaired; everything that could escape the destination is rejected.
fn sanitize_segment(raw: &str) -> Result<String, ApiError> {
    if raw.is_empty() || raw == "." || raw == ".." {
        return Err(ApiError::new("UNSAFE_PATH", format!("unsafe path component {raw:?}")));
    }
    if raw.chars().any(|ch| {
        matches!(ch, ':' | '\\' | '"' | '*' | '?' | '<' | '>' | '|') || (ch as u32) < 0x20
    }) {
        return Err(ApiError::new(
            "UNSAFE_PATH",
            format!("reserved character in {raw:?}"),
        ));
    }
    let trimmed = raw.trim_end_matches([' ', '.']);
    if trimmed.is_empty() {
        return Err(ApiError::new("UNSAFE_PATH", "empty path component"));
    }
    let mut name = trimmed.to_string();
    let stem = name.split('.').next().unwrap_or("").to_ascii_lowercase();
    if RESERVED.contains(&stem.as_str()) {
        name.insert(0, '_');
    }
    if name.encode_utf16().count() > 255 {
        return Err(ApiError::new(
            "PATH_TOO_LONG",
            format!("path component longer than 255 characters: {trimmed}"),
        ));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_and_legacy_names_decode() {
        assert_eq!(decode("相册/照片.txt".as_bytes(), true).unwrap(), "相册\\照片.txt");
        // GBK without the UTF-8 flag, as written by Chinese Windows zip tools. The page is
        // explicit: `decode` starts with the host's page, which is 437/1252 on an English
        // Windows and accepts these bytes as different (mojibake) text.
        assert_eq!(
            decode_bytes_page(&[0xd6, 0xd0, 0xce, 0xc4, b'.', b't', b'x', b't'], 936).unwrap(),
            "中文.txt"
        );
        assert_eq!(decode_bytes_page(&[0xd6, 0xd0], 936).unwrap(), "中");
        assert_eq!(decode_bytes_page(&[0xb0, 0xdb], 437).unwrap(), "░█");
        // Bytes no ANSI page accepts fall back to CP437 instead of failing the entry.
        assert_eq!(cp437_fallback(&[0xff]), "\u{a0}");
        assert_eq!(cp437_fallback(b"ascii-.txt"), "ascii-.txt");
    }

    #[test]
    fn unsafe_names_are_rejected() {
        assert!(decode(b"../evil", false).is_err());
        assert!(decode(b"a/../b", false).is_err());
        assert!(decode(b"/etc/passwd", false).is_err());
        assert!(decode(b"C:/x", false).is_err());
        assert!(decode(b"a/stream:ads", false).is_err());
        assert!(decode(b"a\\..\\b", false).is_err());
        assert!(decode(b"a\0b", false).is_err());
        assert!(decode(b"//", false).is_err());
    }

    #[test]
    fn legacy_trail_bytes_are_not_separators() {
        // CP932/CP936 use 0x5C (`\`) as a trail byte. Splitting the raw name on it would
        // turn `ソ.txt` into `â\.txt`, so the split happens after decoding.
        assert_eq!(decode_bytes_page(&[0x83, 0x5c], 932).unwrap(), "ソ");
        assert_eq!(decode_bytes_page(&[0xd5, 0x5c], 936).unwrap(), "誠");
        assert_eq!(
            decode_parts(&[0x83, 0x5c, b'.', b't', b'x', b't'], false, true, Some(932)).unwrap(),
            "ソ.txt"
        );
        assert_eq!(
            decode_parts(&[0xd5, 0x5c, b'.', b't', b'x', b't'], false, true, Some(936)).unwrap(),
            "誠.txt"
        );
        // A real separator byte still splits once the text is decoded.
        assert_eq!(
            decode_parts(b"dir\\file.txt", false, true, Some(936)).unwrap(),
            "dir\\file.txt"
        );
    }

    #[test]
    fn backslash_separators_become_paths() {
        assert_eq!(decode(b"dir\\file.txt", true).unwrap(), "dir\\file.txt");
        assert_eq!(decode(b"dir\\sub\\file.txt", false).unwrap(), "dir\\sub\\file.txt");
        // Traversal is still refused once the separators are understood.
        assert!(decode(b"dir\\..\\..\\evil", true).is_err());
        assert!(decode(b"dir\\\\file", true).unwrap() == "dir\\file");
    }

    #[test]
    fn device_names_and_trailing_dots_are_repaired() {
        assert_eq!(decode(b"CON.txt", false).unwrap(), "_CON.txt");
        assert_eq!(decode(b"docs/nul", false).unwrap(), "docs\\_nul");
        assert_eq!(decode(b"docs/readme.", false).unwrap(), "docs\\readme");
        assert_eq!(decode(b"docs/console.txt", false).unwrap(), "docs\\console.txt");
    }
}
