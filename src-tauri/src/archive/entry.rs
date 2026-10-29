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
/// `utf8_flag` is the zip bit 11 marker. When clear (and for tar), a name that is valid
/// UTF-8 is still taken as UTF-8 — writers that omit the flag are common and UTF-8 bytes
/// almost never form valid text in a legacy code page. Otherwise the system ANSI code page
/// is tried (what WinRAR/7-Zip do for names produced on a localized Windows) and CP437 is
/// the last resort the zip specification itself names.
pub fn decode(raw: &[u8], utf8_flag: bool) -> Result<String, ApiError> {
    decode_with(raw, utf8_flag, false)
}

/// RAR stores path separators as `\`; every other backend in this crate uses `/`.
pub fn decode_rar(raw: &[u8], utf8_flag: bool) -> Result<String, ApiError> {
    decode_with(raw, utf8_flag, true)
}

fn decode_with(raw: &[u8], utf8_flag: bool, backslash: bool) -> Result<String, ApiError> {
    if raw.starts_with(b"/") || raw.starts_with(b"\\") {
        return Err(ApiError::new("UNSAFE_PATH", "absolute entry name"));
    }
    if raw.contains(&0) {
        return Err(ApiError::new("UNSAFE_PATH", "NUL in entry name"));
    }
    let mut segments = Vec::new();
    let is_separator = |byte: &u8| *byte == b'/' || (backslash && *byte == b'\\');
    for part in raw.split(is_separator) {
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
            decode_legacy(part)
        };
        segments.push(sanitize_segment(&text)?);
    }
    if segments.is_empty() {
        return Err(ApiError::new("INVALID_NAME", "empty entry name"));
    }
    Ok(segments.join("\\"))
}

/// Legacy code page of the user's Windows language, then GBK as the wider net for
/// Chinese archives, then CP437 — the page the zip specification itself falls back to.
///
/// The system ANSI page alone is not enough: machines with the "UTF-8 worldwide language
/// support" beta report 65001, under which every legacy CJK name is invalid.
fn decode_legacy(bytes: &[u8]) -> String {
    for page in [language_codepage(), 936] {
        if let Some(text) = decode_with_page(bytes, page) {
            return text;
        }
    }
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

fn decode_with_page(bytes: &[u8], page: u32) -> Option<String> {
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
        // GBK without the UTF-8 flag, as written by Chinese Windows zip tools.
        assert_eq!(
            decode(&[0xd6, 0xd0, 0xce, 0xc4, b'.', b't', b'x', b't'], false).unwrap(),
            "中文.txt"
        );
        // Explicit pages keep the decoder testable regardless of the machine's language.
        assert_eq!(decode_with_page(&[0xd6, 0xd0], 936).unwrap(), "中");
        assert_eq!(decode_with_page(&[0xb0, 0xdb], 437).unwrap(), "░█");
        // Bytes no ANSI page accepts fall back to CP437 instead of failing the entry.
        assert_eq!(decode(&[0xff], false).unwrap(), "\u{a0}");
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
    fn device_names_and_trailing_dots_are_repaired() {
        assert_eq!(decode(b"CON.txt", false).unwrap(), "_CON.txt");
        assert_eq!(decode(b"docs/nul", false).unwrap(), "docs\\_nul");
        assert_eq!(decode(b"docs/readme.", false).unwrap(), "docs\\readme");
        assert_eq!(decode(b"docs/console.txt", false).unwrap(), "docs\\console.txt");
    }
}
