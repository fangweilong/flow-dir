//! Installed font families for the settings page's family pickers.
//!
//! GDI enumeration is the only source that matches what CSS can select: the font
//! registry lists styles ("Arial Bold") and file entries, which are not family values.
//! `EnumFontFamiliesExW` reports the family name a `font-family` declaration expects.

use std::collections::HashSet;

use windows::Win32::Foundation::LPARAM;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, EnumFontFamiliesExW, DEFAULT_CHARSET, LOGFONTW, TEXTMETRICW,
};

/// Sorted, de-duplicated installed family names.
///
/// Vertical (`@`-prefixed) aliases and names that cannot survive the frontend's CSS
/// quoting rule are dropped, so every returned entry is safe to apply as a family.
pub fn installed_families() -> Vec<String> {
    let mut families = Vec::new();
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return families;
        }
        let logfont = LOGFONTW {
            lfCharSet: DEFAULT_CHARSET,
            ..Default::default()
        };
        let param = LPARAM((&mut families as *mut Vec<String>) as isize);
        let _ = EnumFontFamiliesExW(dc, &logfont, Some(collect_family), param, 0);
        let _ = DeleteDC(dc);
    }

    // One family is reported once per charset, and casing can differ between reports.
    let mut seen = HashSet::new();
    families.retain(|name| seen.insert(name.to_lowercase()));
    families.sort_by_key(|name| name.to_lowercase());
    families
}

unsafe extern "system" fn collect_family(
    logfont: *const LOGFONTW,
    _metrics: *const TEXTMETRICW,
    _font_type: u32,
    lparam: LPARAM,
) -> i32 {
    if let Some(family) = logfont.as_ref().and_then(family_name) {
        let families = &mut *(lparam.0 as *mut Vec<String>);
        families.push(family);
    }
    // Non-zero asks GDI to continue with the next family.
    1
}

/// The LOGFONT face name is a NUL-terminated UTF-16 buffer of `LF_FACESIZE` units.
fn family_name(logfont: &LOGFONTW) -> Option<String> {
    let raw = &logfont.lfFaceName;
    let len = raw.iter().position(|&unit| unit == 0).unwrap_or(raw.len());
    if len == 0 {
        return None;
    }
    let name = String::from_utf16_lossy(&raw[..len]);
    let name = name.trim();
    // `@`-prefixed names are vertical-writing aliases, not selectable families.
    if name.is_empty() || name.starts_with('@') || !valid_family(name) {
        return None;
    }
    Some(name.to_string())
}

/// Mirrors `settings::valid_font_family`: quotes and separators would terminate the
/// generated CSS declaration, so such a family must never be offered in the picker.
fn valid_family(name: &str) -> bool {
    name.len() <= 64
        && !name
            .chars()
            .any(|ch| matches!(ch, '"' | '\'' | '\\' | ';' | '{' | '}' | ',') || ch.is_control())
}

#[cfg(test)]
mod tests {
    #[test]
    fn enumerates_installed_families() {
        let families = super::installed_families();
        assert!(!families.is_empty(), "no font families reported");
        assert!(families.iter().all(|name| !name.starts_with('@')));
        let mut sorted = families.clone();
        sorted.sort_by_key(|name| name.to_lowercase());
        assert_eq!(families, sorted);
    }
}
