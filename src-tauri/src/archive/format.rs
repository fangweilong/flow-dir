//! Archive format detection and the name an extraction should use.
//!
//! The extension decides when it is unambiguous; magic bytes are the fallback for
//! extension-less files. Formats without an in-process backend (rar, iso, lzh) are
//! reported as unsupported instead of being handed to a decoder that cannot read them.

use std::io::Read;

/// Archive layouts this build can extract without an external helper.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Zip,
    SevenZ,
    Cab,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    TarZst,
    Gz,
    Bz2,
    Xz,
    Zst,
}

impl Format {
    pub fn is_tar(self) -> bool {
        matches!(
            self,
            Self::Tar | Self::TarGz | Self::TarBz2 | Self::TarXz | Self::TarZst
        )
    }
}

/// Suffixes in longest-first order so `.tar.gz` is tested before `.gz`.
const SUFFIXES: [(&str, Format); 17] = [
    (".tar.bz2", Format::TarBz2),
    (".tar.zst", Format::TarZst),
    (".tar.gz", Format::TarGz),
    (".tar.xz", Format::TarXz),
    (".tbz2", Format::TarBz2),
    (".tzst", Format::TarZst),
    (".tgz", Format::TarGz),
    (".txz", Format::TarXz),
    (".tar", Format::Tar),
    (".zip", Format::Zip),
    (".7z", Format::SevenZ),
    (".cab", Format::Cab),
    (".bz2", Format::Bz2),
    (".zst", Format::Zst),
    (".gz", Format::Gz),
    (".xz", Format::Xz),
    (".tbz", Format::TarBz2),
];

/// Format implied by the file name alone, if any.
pub fn from_name(name: &str) -> Option<Format> {
    let lower = name.to_ascii_lowercase();
    SUFFIXES
        .iter()
        .find(|(suffix, _)| lower.ends_with(suffix))
        .map(|(_, format)| *format)
}

/// Extracts the archive name without its compression suffixes; `extracted` when nothing
/// usable is left (a file literally named `.zip`).
pub fn output_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let stem = SUFFIXES
        .iter()
        .find(|(suffix, _)| lower.ends_with(suffix))
        .map(|(suffix, _)| &name[..name.len() - suffix.len()])
        .unwrap_or(name);
    let stem = stem.trim_end_matches('.').trim();
    if stem.is_empty() {
        "extracted".to_string()
    } else {
        stem.to_string()
    }
}

/// Format of an archive on disk: the extension first, then the first bytes.
pub fn detect(path: &str) -> Option<Format> {
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    if let Some(format) = from_name(name) {
        return Some(format);
    }
    let mut head = [0u8; 512];
    let read = std::fs::File::open(path).ok()?.read(&mut head).ok()?;
    let bytes = &head[..read];
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        return Some(Format::Zip);
    }
    if bytes.starts_with(&[0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c]) {
        return Some(Format::SevenZ);
    }
    if bytes.starts_with(b"MSCF") {
        return Some(Format::Cab);
    }
    if bytes.starts_with(b"\x1f\x8b") {
        return Some(Format::Gz);
    }
    if bytes.starts_with(b"BZh") {
        return Some(Format::Bz2);
    }
    if bytes.starts_with(b"\xfd7zXZ\0") {
        return Some(Format::Xz);
    }
    if bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        return Some(Format::Zst);
    }
    if bytes.len() >= 262 && &bytes[257..262] == b"ustar" {
        return Some(Format::Tar);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_map_to_formats() {
        assert_eq!(from_name("a.ZIP"), Some(Format::Zip));
        assert_eq!(from_name("a.tar.gz"), Some(Format::TarGz));
        assert_eq!(from_name("a.tgz"), Some(Format::TarGz));
        assert_eq!(from_name("a.tar.bz2"), Some(Format::TarBz2));
        assert_eq!(from_name("a.zst"), Some(Format::Zst));
        assert_eq!(from_name("a.7z"), Some(Format::SevenZ));
        assert_eq!(from_name("a.CAB"), Some(Format::Cab));
        assert_eq!(from_name("a.rar"), None);
        assert_eq!(from_name("a.txt"), None);
    }

    #[test]
    fn output_names_strip_every_suffix() {
        assert_eq!(output_name("photos.zip"), "photos");
        assert_eq!(output_name("backup.tar.gz"), "backup");
        assert_eq!(output_name("data.tbz2"), "data");
        assert_eq!(output_name(".zip"), "extracted");
        assert_eq!(output_name("plain"), "plain");
    }
}
