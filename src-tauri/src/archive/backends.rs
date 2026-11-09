//! Extraction backends: one reader per format family, all of them feeding the same sink.
//!
//! Every backend only decodes; naming, limits, progress, cancellation and the move into
//! the destination live in [`crate::archive::sink`].

use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path as FsPath, PathBuf};

use zip::result::ZipError;

use crate::error::ApiError;

use super::entry;
use super::format::{self, Format};
use super::sink::{Sink, Take};
use super::ArchiveEntry;

/// Decompressed bytes a preview may read before it reports what it has so far.
const PREVIEW_BYTES: u64 = 64 * 1024 * 1024;

/// Budget for materializing a folder inside an archive in the preview cache. Browsing a
/// crafted archive must not be able to fill the disk; explicit extraction passes `None`.
pub const MAX_MATERIALIZE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Entries bound for the same browse path (a tree with millions of tiny files is just a
/// different way to exhaust the cache).
pub const MAX_MATERIALIZE_ENTRIES: usize = 200_000;

/// Refusal marker carried inside the `io::Error` a [`Capped`] writer returns.
#[derive(Debug)]
struct LimitExceeded;

impl std::fmt::Display for LimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("extraction exceeded its byte budget")
    }
}

impl std::error::Error for LimitExceeded {}

/// Writer that refuses bytes past a budget. A member header can claim any size, so the
/// preview path cannot trust it to stop a decompression bomb.
struct Capped<W: Write> {
    inner: W,
    left: u64,
}

impl<W: Write> Write for Capped<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() as u64 > self.left {
            return Err(std::io::Error::other(LimitExceeded));
        }
        self.left -= buf.len() as u64;
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn create_out(dest: &FsPath, limit: Option<u64>) -> Result<Capped<File>, ApiError> {
    let file = File::create(dest).map_err(|e| ApiError::from_io(&dest.to_string_lossy(), &e))?;
    Ok(Capped {
        inner: file,
        left: limit.unwrap_or(u64::MAX),
    })
}

fn hit_limit(err: &std::io::Error) -> bool {
    err.get_ref().is_some_and(|inner| inner.is::<LimitExceeded>())
}

fn write_failed(dest: &FsPath, err: &std::io::Error) -> ApiError {
    if hit_limit(err) {
        return ApiError::new("TOO_LARGE", dest.to_string_lossy());
    }
    ApiError::from_io(&dest.to_string_lossy(), err)
}

/// Opens a decompressing reader for the entry payload of `format`.
fn reader(path: &str, format: Format) -> Result<Box<dyn Read>, ApiError> {
    let file = File::open(path).map_err(|e| ApiError::from_io(path, &e))?;
    let buffered = BufReader::new(file);
    let reader: Box<dyn Read> = match format {
        Format::Tar => Box::new(buffered),
        Format::TarGz => Box::new(flate2::read::GzDecoder::new(buffered)),
        Format::TarBz2 => Box::new(bzip2::read::BzDecoder::new(buffered)),
        Format::TarXz => Box::new(lzma_rust2::XzReader::new(buffered, true)),
        Format::TarZst => Box::new(
            ruzstd::decoding::StreamingDecoder::new(buffered)
                .map_err(|e| ApiError::new("ARCHIVE", e.to_string()))?,
        ),
        Format::Gz => Box::new(flate2::read::GzDecoder::new(buffered)),
        Format::Bz2 => Box::new(bzip2::read::BzDecoder::new(buffered)),
        Format::Xz => Box::new(lzma_rust2::XzReader::new(buffered, true)),
        Format::Zst => Box::new(
            ruzstd::decoding::StreamingDecoder::new(buffered)
                .map_err(|e| ApiError::new("ARCHIVE", e.to_string()))?,
        ),
        Format::Zip | Format::SevenZ | Format::Cab | Format::Rar | Format::Iso => {
            return Err(ApiError::new("INTERNAL", "not a stream format"))
        }
    };
    Ok(reader)
}

pub fn extract(archive: &str, sink: &mut Sink, password: Option<&str>) -> Result<(), ApiError> {
    let format = format::detect(archive)
        .ok_or_else(|| ApiError::new("UNSUPPORTED_FORMAT", name_of(archive)))?;
    match format {
        // Indexed formats know their totals up front; show that pass as its own phase so
        // a huge archive does not look idle.
        Format::Zip => {
            sink.progress_phase("plan");
            extract_zip(archive, sink, password)
        }
        Format::SevenZ => {
            sink.progress_phase("plan");
            extract_7z(archive, sink, password)
        }
        Format::Rar => {
            sink.progress_phase("plan");
            extract_rar(archive, sink, password)
        }
        Format::Iso => {
            sink.progress_phase("plan");
            extract_iso(archive, sink)
        }
        Format::Cab => {
            sink.progress_phase("plan");
            extract_cab(archive, sink)
        }
        format if format.is_tar() => {
            sink.progress_phase("extract");
            extract_tar(reader(archive, format)?, sink)
        }
        format => {
            sink.progress_phase("extract");
            extract_stream(archive, format, sink)
        }
    }
}

fn extract_7z(path: &str, sink: &mut Sink, password: Option<&str>) -> Result<(), ApiError> {
    use sevenz_rust2::{ArchiveReader, Password};

    let mut reader = ArchiveReader::open(path, Password::from(password.unwrap_or("")))
        .map_err(|err| sevenz_error(err, password.is_some()))?;
    let entries: Vec<(String, u64, bool)> = reader
        .archive()
        .files
        .iter()
        .map(|file| (file.name.clone(), file.size, file.is_directory))
        .collect();
    let total: u64 = entries.iter().filter(|(_, _, dir)| !dir).map(|(_, size, _)| *size).sum();
    sink.set_totals(Some(entries.len() as u32), Some(total))?;
    sink.progress_phase("extract");
    let failures_before = sink.failed_total();
    let bytes_before = sink.bytes();

    // The closure can only report sevenz errors, so an abort is parked here and returned
    // once the reader stops (it always stops immediately after).
    let mut abort: Option<ApiError> = None;
    let walked = reader.for_each_entries(|file, read| {
        if abort.is_some() {
            return Ok(false);
        }
        if sink.cancelled() {
            abort = Some(ApiError::new("CANCELED", "extraction canceled"));
            return Ok(false);
        }
        let rel = match entry::decode(file.name.as_bytes(), true) {
            Ok(rel) => rel,
            Err(err) if entry::is_root_marker(&err) => {
                // Still consume a solid archive's member; the name itself is skipped.
                return match std::io::copy(read, &mut std::io::sink()) {
                    Ok(_) => Ok(true),
                    Err(err) => {
                        abort = Some(ApiError::new("ARCHIVE", err.to_string()));
                        Ok(false)
                    }
                };
            }
            Err(err) => {
                sink.skip(&file.name, &err.code);
                // A solid archive stores members back-to-back; leaving this member's
                // bytes unread makes the decoder interpret them as the next member.
                return match std::io::copy(read, &mut std::io::sink()) {
                    Ok(_) => Ok(true),
                    Err(err) => {
                        abort = Some(ApiError::new("ARCHIVE", err.to_string()));
                        Ok(false)
                    }
                };
            }
        };
        match sink.accept(&rel, file.is_directory, false) {
            Ok(Take::Skip) => {
                if file.is_directory {
                    return Ok(true);
                }
                // Skipped file: consume it so the next member starts at its own offset.
                match std::io::copy(read, &mut std::io::sink()) {
                    Ok(_) => Ok(true),
                    Err(err) => {
                        abort = Some(ApiError::new("ARCHIVE", err.to_string()));
                        Ok(false)
                    }
                }
            }
            Ok(Take::Dir) => Ok(true),
            Ok(Take::File(target)) => {
                let result = sink.write(&target, read, file.size);
                // `write` may return Ok after skipping part of the member (create
                // failure, write error, corrupt stream). Draining the remainder keeps a
                // solid 7z stream aligned for every later member.
                let drained = if result.is_ok() {
                    std::io::copy(read, &mut std::io::sink())
                } else {
                    Ok(0)
                };
                match (result, drained) {
                    (Ok(()), Ok(_)) => Ok(true),
                    (Ok(()), Err(err)) => {
                        abort = Some(ApiError::new("ARCHIVE", err.to_string()));
                        Ok(false)
                    }
                    (Err(err), _) => {
                        abort = Some(err);
                        Ok(false)
                    }
                }
            },
            Err(err) => {
                abort = Some(err);
                Ok(false)
            }
        }
    });
    if let Some(err) = abort {
        return Err(err);
    }
    walked.map_err(|err| sevenz_error(err, password.is_some()))?;
    // A wrong password on a plain header decodes garbage that fails every member's CRC.
    if password.is_some() && sink.failed_total() > failures_before && sink.bytes() == bytes_before {
        return Err(ApiError::new("WRONG_PASSWORD", "wrong password"));
    }
    Ok(())
}

/// 7z reports a missing password through the error type, which is the only signal the
/// header gives without decrypting it first.
fn sevenz_error(err: sevenz_rust2::Error, had_password: bool) -> ApiError {
    use sevenz_rust2::Error;
    match err {
        Error::PasswordRequired => ApiError::new(
            if had_password { "WRONG_PASSWORD" } else { "PASSWORD_REQUIRED" },
            "archive header is encrypted",
        ),
        Error::MaybeBadPassword(_) => ApiError::new("WRONG_PASSWORD", "wrong password"),
        other => ApiError::new("ARCHIVE", other.to_string()),
    }
}

/// Opens a local RAR with every sibling volume of its set registered, so a member that
/// starts in one file and ends in another decodes as one stream.
fn open_rar(path: &str, password: Option<&str>) -> Result<unrar_rs::RarArchive, ApiError> {
    use unrar_rs::RarArchive;

    // Only the first volume starts with a RAR signature; picking any other part of the
    // set still has to open from there and then add the rest.
    let set = rar_volume_set(FsPath::new(path)).unwrap_or_default();
    let root = set.first().map_or(FsPath::new(path), PathBuf::as_path);

    let file = File::open(root)
        .map_err(|err| ApiError::from_io(root.to_string_lossy().as_ref(), &err))?;
    let opened = match password {
        Some(password) => RarArchive::open_with_password(file, password),
        None => RarArchive::open(file),
    };
    let mut archive = opened.map_err(|err| rar_error(err, password.is_some()))?;

    for (index, volume) in set.iter().enumerate() {
        if volume == root {
            continue;
        }
        let Ok(reader) = File::open(volume) else {
            // A gap means the later files cannot be addressed either; the volumes that
            // did load still decode every member they cover.
            break;
        };
        if archive.add_volume(index, Box::new(reader)).is_err() {
            break;
        }
    }
    Ok(archive)
}

/// Which naming scheme a RAR volume set follows; derived from any one of its files.
enum VolumeFamily {
    /// `name.part1.rar`, `name.part2.rar`, … (`-vn` in WinRAR/RAR).
    Part(String),
    /// `name.rar` followed by `name.r00`, `name.r01`, … or `name.001`, `name.002`, …
    Sequence { base: String, width: usize },
}

impl VolumeFamily {
    fn of(name: &str) -> Option<Self> {
        let (stem, ext) = name.rsplit_once('.')?;
        let ext = ext.to_ascii_lowercase();
        if ext == "rar" {
            let lower = stem.to_ascii_lowercase();
            if let Some(cut) = lower.rfind(".part") {
                let digits = &stem[cut + 5..];
                if digits.bytes().all(|b| b.is_ascii_digit()) && !digits.is_empty() {
                    return Some(Self::Part(stem[..cut].to_string()));
                }
            }
            return Some(Self::Sequence {
                base: stem.to_string(),
                width: 0,
            });
        }
        if ext.len() > 1 && ext.starts_with('r') && ext[1..].bytes().all(|b| b.is_ascii_digit()) {
            return Some(Self::Sequence {
                base: stem.to_string(),
                width: 0,
            });
        }
        if ext.bytes().all(|b| b.is_ascii_digit()) {
            return Some(Self::Sequence {
                base: stem.to_string(),
                width: ext.len(),
            });
        }
        None
    }

    /// Sort position of `name` inside the set, or `None` when it belongs elsewhere.
    fn key(&self, name: &str) -> Option<u64> {
        let (stem, ext) = name.rsplit_once('.')?;
        let ext = ext.to_ascii_lowercase();
        match self {
            Self::Part(prefix) => {
                if ext != "rar" {
                    return None;
                }
                // `prefix` comes from a different file name in the same folder, so its
                // byte length is not guaranteed to be a character boundary here.
                let head = stem.get(..prefix.len())?;
                if !head.eq_ignore_ascii_case(prefix) {
                    return None;
                }
                let rest = stem[prefix.len()..].to_ascii_lowercase();
                let digits = rest.strip_prefix(".part")?;
                if !rest.starts_with(".part") || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                digits.parse().ok()
            }
            Self::Sequence { base, width } => {
                let same_stem = stem.eq_ignore_ascii_case(base);
                if ext == "rar" && same_stem {
                    return Some(0);
                }
                if *width == 0 {
                    // `name.r00` sorts right after `name.rar`.
                    let digits = ext.strip_prefix('r')?;
                    if !same_stem || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                        return None;
                    }
                    return digits.parse::<u64>().ok().map(|n| n + 1);
                }
                if !same_stem || ext.len() != *width || !ext.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                ext.parse().ok()
            }
        }
    }
}

/// Local volumes of the set `first` belongs to, first volume first. `None` for a plain
/// single-volume archive, so extraction never touches unrelated files.
fn rar_volume_set(first: &FsPath) -> Option<Vec<PathBuf>> {
    let name = first.file_name()?.to_str()?;
    let family = VolumeFamily::of(name)?;
    let mut volumes: Vec<(u64, PathBuf)> = std::fs::read_dir(first.parent()?)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let key = family.key(path.file_name()?.to_str()?)?;
            Some((key, path))
        })
        .collect();
    if volumes.len() < 2 {
        return None;
    }
    volumes.sort_by_key(|(key, _)| *key);
    volumes.dedup_by_key(|(key, _)| *key);
    Some(volumes.into_iter().map(|(_, path)| path).collect())
}

fn extract_rar(path: &str, sink: &mut Sink, password: Option<&str>) -> Result<(), ApiError> {
    let mut archive = open_rar(path, password)?;

    // Headers cost nothing to read, so the password prompt and the totals both come from
    // a preflight pass instead of failing halfway through the member list.
    let mut entries = 0u32;
    let mut total = 0u64;
    let mut encrypted = 0u32;
    for member in archive.entries() {
        entries += 1;
        total += member.unpacked_size.unwrap_or(0);
        if member.is_encrypted {
            encrypted += 1;
        }
    }
    if encrypted > 0 && password.is_none() {
        return Err(ApiError::new("PASSWORD_REQUIRED", "archive is encrypted"));
    }
    sink.set_totals(Some(entries), Some(total))?;
    sink.progress_phase("extract");

    for index in 0..archive.len() {
        if sink.cancelled() {
            return Err(ApiError::new("CANCELED", "extraction canceled"));
        }
        let entry = archive
            .by_index(index)
            .map_err(|err| rar_error(err, password.is_some()))?;
        let info = entry.info();
        // Links would be recreated as ordinary files by the writer we hand over, so they
        // are reported instead of silently turning into copies.
        if info.is_symlink || info.is_hardlink || info.is_file_copy {
            let name = info.name.clone();
            sink.skip(&name, "SYMLINK");
            continue;
        }
        let is_dir = info.is_directory;
        let raw = info
            .raw_name_bytes
            .clone()
            .unwrap_or_else(|| info.name.clone().into_bytes());
        let declared = entry.size().unwrap_or(0);
        let rel = match entry::decode_rar(&raw, false) {
            Ok(rel) => rel,
            Err(err) if entry::is_root_marker(&err) => continue,
            Err(err) => {
                sink.skip(&info.name, &err.code);
                continue;
            }
        };
        if let Take::File(target) = sink.accept(&rel, is_dir, false)? {
            let had_password = password.is_some();
            sink.write_with(&target, declared, move |writer| {
                entry
                    .copy_to(writer)
                    .map(|_| ())
                    .map_err(|err| rar_error(err, had_password))
            })?;
        }
    }
    Ok(())
}

fn rar_error(err: unrar_rs::RarError, had_password: bool) -> ApiError {
    use unrar_rs::RarError;
    match err {
        RarError::EncryptedArchive | RarError::EncryptedMember { .. } => ApiError::new(
            if had_password { "WRONG_PASSWORD" } else { "PASSWORD_REQUIRED" },
            "archive is encrypted",
        ),
        RarError::InvalidPassword | RarError::WrongPassword { .. } => {
            ApiError::new("WRONG_PASSWORD", "wrong password")
        }
        // Decrypting with the wrong key yields garbage that fails the member's own
        // checksum; without a password the same failure means real corruption. The latter
        // is per member, so the sink can skip it and keep the rest of the archive.
        RarError::DataCrcMismatch { .. }
        | RarError::PackedDataCrcMismatch { .. }
        | RarError::Blake2Mismatch { .. }
        | RarError::PackedDataBlake2Mismatch { .. } => {
            if had_password {
                ApiError::new("WRONG_PASSWORD", "wrong password")
            } else {
                ApiError::new("CORRUPT", "member checksum mismatch")
            }
        }
        other => ApiError::new("ARCHIVE", other.to_string()),
    }
}

/// A crafted image can nest directories arbitrarily deep. Recursive traversal would
/// overflow the worker stack, which Rust cannot catch, before any size limit applies.
const MAX_ISO_DEPTH: usize = 512;

/// ISO/UDF images are parsed into a tree first: the format has no random access cost and
/// the tree gives both the totals and the extraction order.
fn extract_iso(path: &str, sink: &mut Sink) -> Result<(), ApiError> {
    let mut file = File::open(path).map_err(|err| ApiError::from_io(path, &err))?;
    let root = isomage::detect_and_parse_filesystem(&mut file, name_of(path))
        .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
    let (entries, bytes) = iso_totals(&root);
    sink.set_totals(Some(entries), Some(bytes))?;
    sink.progress_phase("extract");
    extract_iso_dir(&mut file, root, "", sink, 0)
}

fn iso_totals(node: &isomage::TreeNode) -> (u32, u64) {
    iso_totals_at(node, 0)
}

fn iso_totals_at(node: &isomage::TreeNode, depth: usize) -> (u32, u64) {
    if depth > MAX_ISO_DEPTH {
        return (0, 0);
    }
    let mut entries = 0u32;
    let mut bytes = 0u64;
    for child in &node.children {
        entries += 1;
        if child.is_directory {
            let (nested_entries, nested_bytes) = iso_totals_at(child, depth + 1);
            entries += nested_entries;
            bytes += nested_bytes;
        } else {
            bytes += child.size;
        }
    }
    (entries, bytes)
}

fn extract_iso_dir(
    file: &mut File,
    node: isomage::TreeNode,
    prefix: &str,
    sink: &mut Sink,
    depth: usize,
) -> Result<(), ApiError> {
    if depth > MAX_ISO_DEPTH {
        return Err(ApiError::new("ARCHIVE", "ISO directory nesting is too deep"));
    }
    for child in &node.children {
        if sink.cancelled() {
            return Err(ApiError::new("CANCELED", "extraction canceled"));
        }
        let raw = if prefix.is_empty() {
            child.name.clone()
        } else {
            format!("{prefix}/{}", child.name)
        };
        let rel = match entry::decode(raw.as_bytes(), true) {
            Ok(rel) => rel,
            Err(err) if entry::is_root_marker(&err) => continue,
            Err(err) => {
                sink.skip(&raw, &err.code);
                continue;
            }
        };
        if child.is_directory {
            sink.accept(&rel, true, false)?;
            extract_iso_dir(file, child.clone(), &raw, sink, depth + 1)?;
        } else if let Take::File(target) = sink.accept(&rel, false, false)? {
            let size = child.size;
            sink.write_with(&target, size, |writer| {
                // A damaged member is reported and skipped; an unreadable image fails every
                // member and leaves the sink's failure list to say so.
                isomage::cat_node(file, child, writer)
                    .map_err(|err| ApiError::new("CORRUPT", err.to_string()))
            })?;
        }
    }
    Ok(())
}

/// Cabinets carry no directory entries; the sink creates parents from each file path.
fn extract_cab(path: &str, sink: &mut Sink) -> Result<(), ApiError> {
    let file = File::open(path).map_err(|err| ApiError::from_io(path, &err))?;
    let mut cabinet = cab::Cabinet::new(file).map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
    let mut entries: Vec<(String, u64, bool)> = Vec::new();
    for folder in cabinet.folder_entries() {
        for file in folder.file_entries() {
            entries.push((
                file.name().to_string(),
                file.uncompressed_size() as u64,
                file.is_name_utf(),
            ));
        }
    }
    let total: u64 = entries.iter().map(|(_, size, _)| *size).sum();
    sink.set_totals(Some(entries.len() as u32), Some(total))?;
    sink.progress_phase("extract");

    for (name, size, utf8) in entries {
        // CAB stores path separators as `\` (MS-CAB, CFILE.szFile).
        let rel = match entry::decode_rar(name.as_bytes(), utf8) {
            Ok(rel) => rel,
            Err(err) if entry::is_root_marker(&err) => continue,
            Err(err) => {
                sink.skip(&name, &err.code);
                continue;
            }
        };
        let Take::File(target) = sink.accept(&rel, false, false)? else {
            continue;
        };
        let mut reader = cabinet
            .read_file(&name)
            .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
        sink.write(&target, &mut reader, size)?;
    }
    Ok(())
}

fn extract_zip(path: &str, sink: &mut Sink, password: Option<&str>) -> Result<(), ApiError> {
    let file = File::open(path).map_err(|e| ApiError::from_io(path, &e))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| ApiError::new("ARCHIVE", e.to_string()))?;
    // The central directory carries the flags and sizes; opening entries here would
    // already need the password for AES members.
    let metadata = archive.metadata();
    let mut total = 0u64;
    let mut encrypted = 0u32;
    for index in 0..metadata.len() {
        if let Ok(entry) = metadata.entry(index) {
            total += entry.size();
            if entry.flags().is_encrypted() {
                encrypted += 1;
            }
        }
    }
    // Ask before writing anything: a partially extracted tree that then fails on the
    // first encrypted member is worse than a prompt.
    if encrypted > 0 && password.is_none() {
        return Err(ApiError::new("PASSWORD_REQUIRED", "archive is encrypted"));
    }
    sink.set_totals(Some(archive.len() as u32), Some(total))?;
    sink.progress_phase("extract");
    let failures_before = sink.failed_total();
    let bytes_before = sink.bytes();

    for index in 0..archive.len() {
        if sink.cancelled() {
            return Err(ApiError::new("CANCELED", "extraction canceled"));
        }
        let opened = match password {
            Some(password) => archive.by_index_decrypt(index, password.as_bytes()),
            None => archive.by_index(index),
        };
        let mut file = match opened {
            Ok(file) => file,
            Err(ZipError::InvalidPassword) => {
                return Err(ApiError::new("WRONG_PASSWORD", "wrong password"))
            }
            Err(_) => {
                sink.skip(&format!("entry #{index}"), "CORRUPT");
                continue;
            }
        };
        let raw = file.name_raw().to_vec();
        let utf8 = file.flags().as_u16() & 0x800 != 0;
        let rel = match entry::decode(&raw, utf8) {
            Ok(rel) => rel,
            Err(err) if entry::is_root_marker(&err) => continue,
            Err(err) => {
                sink.skip(&String::from_utf8_lossy(&raw), &err.code);
                continue;
            }
        };
        let declared = file.size();
        let is_dir = file.is_dir();
        let link = file.is_symlink();
        match sink.accept(&rel, is_dir, link)? {
            Take::Skip | Take::Dir => {}
            Take::File(target) => {
                sink.write(&target, &mut file, declared)?;
            }
        }
    }
    // ZipCrypto has no verifier: a wrong password only shows as a failed member and no
    // bytes written, which would otherwise be reported as plain corruption.
    if password.is_some() && sink.failed_total() > failures_before && sink.bytes() == bytes_before {
        return Err(ApiError::new("WRONG_PASSWORD", "wrong password"));
    }
    Ok(())
}

fn extract_tar(reader: Box<dyn Read>, sink: &mut Sink) -> Result<(), ApiError> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive
        .entries()
        .map_err(|e| ApiError::new("ARCHIVE", e.to_string()))?;
    for entry in entries {
        if sink.cancelled() {
            return Err(ApiError::new("CANCELED", "extraction canceled"));
        }
        // A damaged tail should not discard what already came out of the stream, but it
        // must be reported: silently breaking made a truncated archive look complete.
        let mut entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                log::debug!(target: "flowdir::archive", "tar stream stopped: {err}");
                sink.skip("<archive stream>", "CORRUPT");
                break;
            }
        };
        let header = entry.header();
        let kind = header.entry_type();
        let is_dir = kind.is_dir();
        let link = kind.is_symlink() || kind.is_hard_link();
        let declared = header.size().unwrap_or(0);
        let raw = entry.path_bytes().to_vec();
        let rel = match entry::decode(&raw, false) {
            Ok(rel) => rel,
            Err(err) if entry::is_root_marker(&err) => continue,
            Err(err) => {
                sink.skip(&String::from_utf8_lossy(&raw), &err.code);
                continue;
            }
        };
        match sink.accept(&rel, is_dir, link)? {
            Take::Skip | Take::Dir => {}
            Take::File(target) => sink.write(&target, &mut entry, declared)?,
        }
    }
    Ok(())
}

/// Bare `.gz`/`.bz2`/`.xz`/`.zst`: one payload, named after the header or the archive.
fn extract_stream(archive: &str, format: Format, sink: &mut Sink) -> Result<(), ApiError> {
    // gzip is decoded here rather than through `reader` so the original name in the
    // header is available; `a.txt.gz` loses nothing, `data.gz` gains the real name.
    if format == Format::Gz {
        let file = File::open(archive).map_err(|e| ApiError::from_io(archive, &e))?;
        let mut decoder = flate2::read::GzDecoder::new(file);
        let mut name = format::output_name(name_of(archive));
        if let Some(inner) = decoder.header().and_then(|header| header.filename()) {
            if let Ok(decoded) = entry::decode(inner, false) {
                name = decoded;
            }
        }
        sink.set_totals(Some(1), None)?;
        if let Take::File(target) = sink.accept(&name, false, false)? {
            sink.write(&target, &mut decoder, 0)?;
        }
        return Ok(());
    }
    let mut reader = reader(archive, format)?;
    let name = format::output_name(name_of(archive));
    sink.set_totals(Some(1), None)?;
    if let Take::File(target) = sink.accept(&name, false, false)? {
        sink.write(&target, &mut reader, 0)?;
    }
    Ok(())
}

/// Entries for the preview panel. Listing is capped by the caller; the tar variants stop
/// decompressing as soon as the cap is reached.
///
/// The boolean reports whether the archive held more members than the cap allowed, so a
/// virtual directory listing can surface truncation instead of silently dropping entries.
pub fn list_with_truncated(
    path: &str,
    limit: usize,
) -> Result<(Vec<ArchiveEntry>, bool), ApiError> {
    list_with_truncated_impl(path, limit, true)
}

/// Same listing without the preview decompression budget. Extraction must not reuse the
/// preview cap: stopping a tar walk at 64 MiB would silently omit every later member while
/// the caller still reports a complete directory.
pub fn list_unbounded_with_truncated(
    path: &str,
    limit: usize,
) -> Result<(Vec<ArchiveEntry>, bool), ApiError> {
    list_with_truncated_impl(path, limit, false)
}

fn list_with_truncated_impl(
    path: &str,
    limit: usize,
    preview: bool,
) -> Result<(Vec<ArchiveEntry>, bool), ApiError> {
    let format = format::detect(path).ok_or_else(|| ApiError::new("UNSUPPORTED_FORMAT", path))?;
    if format == Format::Zip {
        let file = File::open(path).map_err(|e| ApiError::from_io(path, &e))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| ApiError::new("ARCHIVE", e.to_string()))?;
        let total = archive.len();
        let mut entries = Vec::new();
        for index in 0..total.min(limit) {
            let Ok(file) = archive.by_index(index) else {
                continue;
            };
            let raw = file.name_raw();
            let utf8 = file.flags().as_u16() & 0x800 != 0;
            let is_dir = file.is_dir() || raw.ends_with(b"/") || raw.ends_with(b"\\");
            if let Ok(name) = entry::decode(raw, utf8) {
                entries.push(ArchiveEntry {
                    name,
                    size: if is_dir { 0 } else { file.size() },
                    is_dir,
                    mtime: 0,
                });
            }
        }
        return Ok((entries, total > limit));
    }
    if format == Format::SevenZ {
        use sevenz_rust2::{ArchiveReader, Password};
        let reader = ArchiveReader::open(path, Password::empty())
            .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
        let files: Vec<_> = reader
            .archive()
            .files
            .iter()
            .filter(|file| !file.name.is_empty())
            .collect();
        let truncated = files.len() > limit;
        let entries = files
            .into_iter()
            .take(limit)
            .filter_map(|file| {
                let is_dir = file.is_directory;
                entry::decode(file.name.as_bytes(), true)
                    .ok()
                    .map(|name| ArchiveEntry {
                        name,
                        size: if is_dir { 0 } else { file.size },
                        is_dir,
                        mtime: 0,
                    })
            })
            .collect();
        return Ok((entries, truncated));
    }
    if format == Format::Cab {
        let file = File::open(path).map_err(|err| ApiError::from_io(path, &err))?;
        let cabinet =
            cab::Cabinet::new(file).map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
        let mut entries = Vec::new();
        for folder in cabinet.folder_entries() {
            for item in folder.file_entries() {
                if entries.len() >= limit {
                    // Reached the cap with this item still unvisited: there is more to show.
                    return Ok((entries, true));
                }
                if let Ok(name) = entry::decode_rar(item.name().as_bytes(), item.is_name_utf()) {
                    entries.push(ArchiveEntry {
                        name,
                        size: item.uncompressed_size() as u64,
                        is_dir: false,
                        mtime: 0,
                    });
                }
            }
        }
        return Ok((entries, false));
    }
    if format == Format::Rar {
        let archive = open_rar(path, None)?;
        let mut entries = Vec::new();
        let mut truncated = false;
        for member in archive.entries() {
            if entries.len() >= limit {
                // The cap was reached before this member, so the archive holds more.
                truncated = true;
                break;
            }
            let raw = member
                .raw_name_bytes
                .clone()
                .unwrap_or_else(|| member.name.clone().into_bytes());
            if let Ok(name) = entry::decode_rar(&raw, false) {
                let is_dir = member.is_directory;
                entries.push(ArchiveEntry {
                    name,
                    size: if is_dir { 0 } else { member.unpacked_size.unwrap_or(0) },
                    is_dir,
                    mtime: 0,
                });
            }
        }
        return Ok((entries, truncated));
    }
    if format == Format::Iso {
        let mut file = File::open(path).map_err(|err| ApiError::from_io(path, &err))?;
        let root = isomage::detect_and_parse_filesystem(&mut file, name_of(path))
            .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
        let mut entries = Vec::new();
        let truncated = push_iso_entries(&root, "", limit, 0, &mut entries);
        return Ok((entries, truncated));
    }
    if format.is_tar() {
        let mut entries = Vec::new();
        // Listing a compressed tar means decompressing it; previews stop after a bounded
        // amount, but extraction walks the whole stream.
        let stream: Box<dyn std::io::Read> = if preview {
            Box::new(reader(path, format)?.take(PREVIEW_BYTES))
        } else {
            Box::new(reader(path, format)?)
        };
        let mut archive = tar::Archive::new(stream);
        let mut items = archive
            .entries()
            .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
        let mut visited = 0usize;
        let mut truncated = false;
        for item in items.by_ref().take(limit) {
            visited += 1;
            let item = match item {
                Ok(item) => item,
                // A preview stops the stream at `PREVIEW_BYTES`, which the tar reader
                // reports as an entry error when the cut lands mid-entry. That is the
                // budget, not corruption: keep the entries read so far and say so.
                Err(_) if preview => {
                    truncated = true;
                    break;
                }
                // The unbounded walk used by extraction cannot run out of budget. A broken
                // entry means the rest of the stream is unreadable, and ending the walk
                // here would report a clean, complete prefix that extraction then caches.
                Err(err) => return Err(ApiError::new("ARCHIVE", err.to_string())),
            };
            let raw = item.path_bytes().to_vec();
            let Ok(name) = entry::decode(&raw, false) else {
                continue;
            };
            let is_dir = item.header().entry_type().is_dir();
            // GNU base-256 tar headers can encode the full u64 range; an unchecked
            // multiply panics in debug builds on a crafted member.
            let mtime = item.header().mtime().unwrap_or(0).saturating_mul(1000);
            entries.push(ArchiveEntry {
                name,
                size: if is_dir { 0 } else { item.header().size().unwrap_or(0) },
                is_dir,
                mtime,
            });
        }
        if visited == limit {
            // The cap stopped the walk: one more header shows whether anything was hidden.
            // `PREVIEW_BYTES` still bounds the decompressor, so this stays cheap.
            truncated = items.next().is_some();
        }
        return Ok((entries, truncated));
    }
    Ok((
        vec![ArchiveEntry {
            name: format::output_name(name_of(path)),
            size: 0,
            is_dir: false,
            mtime: 0,
        }],
        false,
    ))
}

/// Whether a tar archive contains `target` or any entry beneath it.
///
/// Returns `None` for other formats so the caller keeps using the full listing. A
/// compressed tar has no index, but the walk stops at the first match: answering "is this
/// a directory?" must not decompress an entire multi-gigabyte archive.
pub fn tar_contains_entry(
    path: &str,
    target: &str,
    limit: usize,
) -> Result<Option<bool>, ApiError> {
    let Some(format) = format::detect(path) else {
        return Ok(None);
    };
    if !format.is_tar() {
        return Ok(None);
    }
    let target = target.trim_matches('\\').to_lowercase();
    if target.is_empty() {
        return Ok(Some(true));
    }
    let prefix = format!("{target}\\");
    let mut archive = tar::Archive::new(reader(path, format)?);
    let items = archive
        .entries()
        .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
    for (index, item) in items.enumerate() {
        if index >= limit {
            return Err(ApiError::new(
                "TOO_MANY_ENTRIES",
                format!("more than {limit} entries"),
            ));
        }
        // A broken stream cannot be inspected further; surfacing the error is better than
        // guessing that the entry is not a directory.
        let item = item.map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
        let raw = item.path_bytes().to_vec();
        let Ok(name) = entry::decode(&raw, false) else {
            continue;
        };
        let norm = name.trim_matches('\\').to_lowercase();
        if norm.starts_with(&prefix)
            || (norm == target && item.header().entry_type().is_dir())
        {
            return Ok(Some(true));
        }
    }
    Ok(Some(false))
}

/// Drop-in for callers that only need the entries and ignore truncation.
pub fn list(path: &str, limit: usize) -> Result<Vec<ArchiveEntry>, ApiError> {
    list_with_truncated(path, limit).map(|(entries, _)| entries)
}

/// Appends `node`'s descendants, stopping at `limit`. Returns true when the cap hid children.
fn push_iso_entries(
    node: &isomage::TreeNode,
    prefix: &str,
    limit: usize,
    depth: usize,
    out: &mut Vec<ArchiveEntry>,
) -> bool {
    // The extraction side already caps traversal depth; browsing needs the same guard or
    // a crafted image can exhaust the stack.
    if depth >= MAX_ISO_DEPTH {
        return true;
    }
    for child in &node.children {
        if out.len() >= limit {
            return true;
        }
        let raw = if prefix.is_empty() {
            child.name.clone()
        } else {
            format!("{prefix}/{}", child.name)
        };
        if let Ok(name) = entry::decode(raw.as_bytes(), true) {
            let is_dir = child.is_directory;
            out.push(ArchiveEntry {
                name,
                size: if is_dir { 0 } else { child.size },
                is_dir,
                mtime: 0,
            });
        }
        if child.is_directory && push_iso_entries(child, &raw, limit, depth + 1, out) {
            return true;
        }
    }
    false
}

/// Extracts the directory `inner` from `archive` under `dest`, creating the tree.
///
/// Directory members become folders; only file members are written, so an explicit
/// `folder/sub` record can no longer turn into a zero-byte file that blocks its own
/// children. With `uniquify`, an existing file of the same name is renamed instead of
/// being truncated, matching the copy path's collision policy.
pub fn extract_dir(
    archive: &str,
    inner: &str,
    dest: &std::path::Path,
    uniquify: bool,
    limit: Option<u64>,
) -> Result<Vec<String>, ApiError> {
    // A zip central directory allows one pass; the generic loop below re-opens the archive
    // for every member, which is quadratic for a big folder dragged out of a big zip.
    if format::detect(archive) == Some(Format::Zip) {
        return extract_zip_dir(archive, inner, dest, uniquify, limit);
    }
    let mut written = Vec::new();
    let mut total = 0u64;
    let mut entries = 0usize;
    // Extraction uses the unbounded listing: a truncated preview listing would drop every
    // member past the cap and still look successful.
    let (listed, truncated) = list_unbounded_with_truncated(archive, 500_000)?;
    if truncated {
        return Err(ApiError::new(
            "TOO_MANY_ENTRIES",
            format!("more than {MAX_MATERIALIZE_ENTRIES} entries"),
        ));
    }
    for entry in listed {
        let norm = entry.name.replace('/', "\\");
        let norm = norm.trim_matches('\\');
        let Some(rel) = crate::pathkey::strip_prefix_dir(norm, inner) else {
            continue;
        };
        let rel = rel.trim_matches('\\');
        if rel.is_empty() {
            continue;
        }
        // Count directories too: a tree of empty folders is another way to exhaust the
        // cache, and `written` only tracks files.
        entries += 1;
        if limit.is_some() && entries > MAX_MATERIALIZE_ENTRIES {
            return Err(ApiError::new(
                "TOO_MANY_ENTRIES",
                format!("more than {MAX_MATERIALIZE_ENTRIES} entries"),
            ));
        }
        let target = dest.join(rel);
        if entry.is_dir {
            std::fs::create_dir_all(&target)
                .map_err(|e| ApiError::from_io(&target.to_string_lossy(), &e))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ApiError::from_io(&parent.to_string_lossy(), &e))?;
        }
        let target = if uniquify {
            let path = target.to_string_lossy().into_owned();
            std::path::PathBuf::from(crate::naming::unique(std::slice::from_ref(&path), &[]).remove(0))
        } else {
            target
        };
        let remaining = limit.map(|cap| cap.saturating_sub(total));
        // A zero-byte member still fits an exhausted budget; a lying header is caught by
        // the Capped writer inside `extract_single`.
        if remaining == Some(0) && entry.size > 0 {
            return Err(ApiError::new("TOO_LARGE", inner.to_string()));
        }
        extract_single(archive, norm, &target, None, remaining)?;
        total = total.saturating_add(std::fs::metadata(&target).map(|meta| meta.len()).unwrap_or(0));
        written.push(target.to_string_lossy().into_owned());
    }
    Ok(written)
}

/// One-pass folder extraction for zip: the archive is opened once and every matching
/// member is streamed to disk in central-directory order.
fn extract_zip_dir(
    archive: &str,
    inner: &str,
    dest: &std::path::Path,
    uniquify: bool,
    limit: Option<u64>,
) -> Result<Vec<String>, ApiError> {
    let inner_norm = inner.replace('/', "\\");
    let inner_norm = inner_norm.trim_matches('\\');
    let file = File::open(archive).map_err(|e| ApiError::from_io(archive, &e))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| ApiError::new("ARCHIVE", e.to_string()))?;
    let mut written = Vec::new();
    let mut total = 0u64;
    let mut entries = 0usize;
    let mut first_error: Option<ApiError> = None;
    for index in 0..zip.len() {
        // Resolve the name before `by_index`: the member handle borrows the archive, so a
        // lookup from the failure arm is rejected by the borrow checker. The central
        // directory is already in memory, so this is a cheap name decode.
        // Unknown names count as inside: a failure we cannot attribute must still be
        // reported rather than silently dropped.
        let inside = zip
            .name_for_index(index)
            .and_then(Result::ok)
            .map(|name| {
                let name = name.replace('/', "\\");
                crate::pathkey::strip_prefix_dir(&name, inner_norm).is_some()
            })
            .unwrap_or(true);
        // Encrypted or damaged members are skipped like the generic path does; the folder
        // is still extracted with everything readable.
        let mut member = match zip.by_index(index) {
            Ok(member) => member,
            Err(err) => {
                // Only a failure under the requested subtree makes this folder incomplete.
                // An unrelated member (encrypted, unsupported method) must not fail a
                // folder whose own members all extracted.
                // Remember the first inside failure: if nothing at all could be opened (an
                // encrypted folder dragged without a password), reporting success would
                // hand the user an empty folder instead.
                if inside && first_error.is_none() {
                    first_error = Some(match err {
                        ZipError::UnsupportedArchive(_) => {
                            ApiError::new("PASSWORD_REQUIRED", "archive is encrypted")
                        }
                        other => ApiError::new("ARCHIVE", other.to_string()),
                    });
                }
                continue;
            }
        };
        let raw = member.name_raw().to_vec();
        let utf8 = member.flags().as_u16() & 0x800 != 0;
        let Ok(name) = entry::decode(&raw, utf8) else {
            continue;
        };
        let Some(rel) = crate::pathkey::strip_prefix_dir(&name, inner_norm) else {
            continue;
        };
        let rel = rel.trim_matches('\\');
        if rel.is_empty() {
            continue;
        }
        entries += 1;
        if limit.is_some() && entries > MAX_MATERIALIZE_ENTRIES {
            return Err(ApiError::new(
                "TOO_MANY_ENTRIES",
                format!("more than {MAX_MATERIALIZE_ENTRIES} entries"),
            ));
        }
        let target = dest.join(rel);
        if member.is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|e| ApiError::from_io(&target.to_string_lossy(), &e))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ApiError::from_io(&parent.to_string_lossy(), &e))?;
        }
        let target = if uniquify {
            let path = target.to_string_lossy().into_owned();
            std::path::PathBuf::from(crate::naming::unique(std::slice::from_ref(&path), &[]).remove(0))
        } else {
            target
        };
        let mut out = File::create(&target)
            .map_err(|e| ApiError::from_io(&target.to_string_lossy(), &e))?;
        // Enforce the browse budget on the stream, not on the header: a member can claim
        // a tiny size and still decompress to gigabytes.
        let copied = match limit.map(|cap| cap.saturating_sub(total)) {
            None => std::io::copy(&mut member, &mut out)
                .map_err(|e| ApiError::from_io(&target.to_string_lossy(), &e))?,
            // `left` may be 0: `take(1)` then proves whether the member is really empty.
            Some(left) => {
                let copied = std::io::copy(&mut (&mut member).take(left.saturating_add(1)), &mut out)
                    .map_err(|e| ApiError::from_io(&target.to_string_lossy(), &e))?;
                if copied > left {
                    drop(out);
                    let _ = std::fs::remove_file(&target);
                    return Err(ApiError::new("TOO_LARGE", inner.to_string()));
                }
                copied
            }
        };
        out.flush().map_err(|e| ApiError::from_io(&target.to_string_lossy(), &e))?;
        total = total.saturating_add(copied);
        written.push(target.to_string_lossy().into_owned());
    }
    // Any member that could not be opened makes the tree incomplete. Report it even when
    // other members landed, so a caller never caches a partial folder as complete.
    if let Some(err) = first_error {
        return Err(err);
    }
    Ok(written)
}

/// Extracts a single entry from `archive` into `dest`.
///
/// `limit` caps the bytes written (previews pass the configured preview size); member
/// headers are untrusted, so the cap is enforced on the stream, not on the declared size.
pub fn extract_single(
    archive: &str,
    inner: &str,
    dest: &std::path::Path,
    password: Option<&str>,
    limit: Option<u64>,
) -> Result<(), ApiError> {
    let format = format::detect(archive)
        .ok_or_else(|| ApiError::new("UNSUPPORTED_FORMAT", name_of(archive)))?;
    let inner_norm = inner.replace('/', "\\");
    let inner_norm = inner_norm.trim_matches('\\');

    match format {
        Format::Zip => {
            let file = File::open(archive).map_err(|e| ApiError::from_io(archive, &e))?;
            let mut zip = zip::ZipArchive::new(file).map_err(|e| ApiError::new("ARCHIVE", e.to_string()))?;
            for index in 0..zip.len() {
                let opened = match password {
                    Some(pwd) => zip.by_index_decrypt(index, pwd.as_bytes()),
                    None => zip.by_index(index),
                };
                let Ok(mut member) = opened else { continue; };
                let raw = member.name_raw().to_vec();
                let utf8 = member.flags().as_u16() & 0x800 != 0;
                if let Ok(name) = entry::decode(&raw, utf8) {
                    if name.eq_ignore_ascii_case(inner_norm) {
                        let mut out = create_out(dest, limit)?;
                        std::io::copy(&mut member, &mut out).map_err(|e| write_failed(dest, &e))?;
                        out.flush().map_err(|e| write_failed(dest, &e))?;
                        return Ok(());
                    }
                }
            }
            Err(ApiError::new("NOT_FOUND", inner))
        }
        Format::SevenZ => {
            use sevenz_rust2::{ArchiveReader, Password};
            let mut reader = ArchiveReader::open(archive, Password::from(password.unwrap_or("")))
                .map_err(|err| sevenz_error(err, password.is_some()))?;
            let mut found = false;
            let mut copy_err: Option<ApiError> = None;
            let _ = reader.for_each_entries(|file, read| {
                if let Ok(name) = entry::decode(file.name.as_bytes(), true) {
                    if name.eq_ignore_ascii_case(inner_norm) {
                        found = true;
                        match create_out(dest, limit) {
                            Ok(mut out) => {
                                if let Err(e) = std::io::copy(read, &mut out).and_then(|_| out.flush()) {
                                    copy_err = Some(write_failed(dest, &e));
                                }
                            }
                            Err(e) => copy_err = Some(e),
                        }
                        return Ok(false);
                    }
                }
                // Solid 7z members are stored back-to-back. Skipping one without
                // consuming its bytes makes the decoder treat the remainder as the next
                // member (wrong file contents, or a CRC failure on every later entry).
                match std::io::copy(read, &mut std::io::sink()) {
                    Ok(_) => Ok(true),
                    Err(err) => {
                        copy_err = Some(ApiError::new("ARCHIVE", err.to_string()));
                        Ok(false)
                    }
                }
            });
            if let Some(err) = copy_err {
                return Err(err);
            }
            if found {
                return Ok(());
            }
            Err(ApiError::new("NOT_FOUND", inner))
        }
        format if matches!(format, Format::Gz | Format::Bz2 | Format::Xz | Format::Zst) => {
            // A bare compressed stream holds exactly one member, named after the archive
            // without its compression suffix. The listing shows that pseudo entry, so
            // opening or dragging it must work too.
            let expected = format::output_name(name_of(archive));
            if !expected.eq_ignore_ascii_case(inner_norm) {
                return Err(ApiError::new("NOT_FOUND", inner));
            }
            let mut src = reader(archive, format)?;
            let mut out = create_out(dest, limit)?;
            std::io::copy(&mut src, &mut out).map_err(|e| write_failed(dest, &e))?;
            out.flush().map_err(|e| write_failed(dest, &e))?;
            Ok(())
        }
        format if format.is_tar() => {
            let mut tar = tar::Archive::new(reader(archive, format)?);
            let items = tar.entries().map_err(|e| ApiError::new("ARCHIVE", e.to_string()))?;
            for item in items {
                let Ok(mut item) = item else { continue; };
                let raw = item.path_bytes().to_vec();
                if let Ok(name) = entry::decode(&raw, false) {
                    if name.eq_ignore_ascii_case(inner_norm) {
                        let mut out = create_out(dest, limit)?;
                        std::io::copy(&mut item, &mut out).map_err(|e| write_failed(dest, &e))?;
                        out.flush().map_err(|e| write_failed(dest, &e))?;
                        return Ok(());
                    }
                }
            }
            Err(ApiError::new("NOT_FOUND", inner))
        }
        Format::Cab => {
            let file = File::open(archive).map_err(|err| ApiError::from_io(archive, &err))?;
            let mut cabinet = cab::Cabinet::new(file).map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
            let mut found_file = None;
            for folder in cabinet.folder_entries() {
                for item in folder.file_entries() {
                    if let Ok(name) = entry::decode_rar(item.name().as_bytes(), item.is_name_utf()) {
                        if name.eq_ignore_ascii_case(inner_norm) {
                            found_file = Some(item.name().to_string());
                            break;
                        }
                    }
                }
                if found_file.is_some() {
                    break;
                }
            }
            if let Some(file_name) = found_file {
                let mut reader = cabinet.read_file(&file_name).map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
                let mut out = create_out(dest, limit)?;
                std::io::copy(&mut reader, &mut out).map_err(|e| write_failed(dest, &e))?;
                out.flush().map_err(|e| write_failed(dest, &e))?;
                return Ok(());
            }
            Err(ApiError::new("NOT_FOUND", inner))
        }
        Format::Rar => {
            let mut rar = open_rar(archive, password)?;
            for index in 0..rar.len() {
                let Ok(entry) = rar.by_index(index) else { continue; };
                let info = entry.info();
                let raw = info.raw_name_bytes.clone().unwrap_or_else(|| info.name.clone().into_bytes());
                if let Ok(name) = entry::decode_rar(&raw, false) {
                    if name.eq_ignore_ascii_case(inner_norm) {
                        let mut out = create_out(dest, limit)?;
                        let had_password = password.is_some();
                        match entry.copy_to(&mut out) {
                            Ok(_) => {
                                out.flush().map_err(|e| write_failed(dest, &e))?;
                                return Ok(());
                            }
                            Err(unrar_rs::RarError::Io(io)) if hit_limit(&io) => {
                                return Err(ApiError::new("TOO_LARGE", dest.to_string_lossy()));
                            }
                            Err(err) => return Err(rar_error(err, had_password)),
                        }
                    }
                }
            }
            Err(ApiError::new("NOT_FOUND", inner))
        }
        Format::Iso => {
            let mut file = File::open(archive).map_err(|err| ApiError::from_io(archive, &err))?;
            let root = isomage::detect_and_parse_filesystem(&mut file, name_of(archive))
                .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
            if extract_iso_single(&mut file, &root, "", inner_norm, dest, limit, 0)? {
                return Ok(());
            }
            Err(ApiError::new("NOT_FOUND", inner))
        }
        _ => Err(ApiError::new("UNSUPPORTED_FORMAT", name_of(archive))),
    }
}

fn extract_iso_single(
    file: &mut File,
    node: &isomage::TreeNode,
    prefix: &str,
    target: &str,
    dest: &std::path::Path,
    limit: Option<u64>,
    depth: usize,
) -> Result<bool, ApiError> {
    if depth > MAX_ISO_DEPTH {
        return Err(ApiError::new("ARCHIVE", "ISO directory nesting is too deep"));
    }
    for child in &node.children {
        let raw = if prefix.is_empty() {
            child.name.clone()
        } else {
            format!("{prefix}/{}", child.name)
        };
        if let Ok(name) = entry::decode(raw.as_bytes(), true) {
            if child.is_directory {
                if extract_iso_single(file, child, &raw, target, dest, limit, depth + 1)? {
                    return Ok(true);
                }
            } else if name.eq_ignore_ascii_case(target) {
                let mut out = create_out(dest, limit)?;
                match isomage::cat_node(file, child, &mut out) {
                    Ok(()) => return Ok(true),
                    Err(err) => {
                        if err
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(hit_limit)
                        {
                            return Err(ApiError::new("TOO_LARGE", dest.to_string_lossy()));
                        }
                        return Err(ApiError::new("ARCHIVE", err.to_string()));
                    }
                }
            }
        }
    }
    Ok(false)
}

fn name_of(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::{
        extract_dir, list, list_unbounded_with_truncated, list_with_truncated, tar_contains_entry,
        VolumeFamily,
    };
    use std::io::Write;

    #[test]
    fn extract_dir_builds_folders_and_never_truncates() {
        let dir = std::env::temp_dir().join(format!("flowdir-extract-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("tree.zip");
        let file = std::fs::File::create(&archive).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        // Explicit directory records, as Explorer and 7-Zip write them: they used to be
        // extracted as zero-byte files that then blocked their own children.
        writer.add_directory("folder/sub/", options).unwrap();
        writer.start_file("folder/sub/a.txt", options).unwrap();
        writer.write_all(b"child").unwrap();
        writer.start_file("folder/top.txt", options).unwrap();
        writer.write_all(b"top").unwrap();
        writer.finish().unwrap();

        let dest = dir.join("out");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("top.txt"), b"keep").unwrap();

        let written = extract_dir(&archive.to_string_lossy(), "folder", &dest, true, None).unwrap();
        assert_eq!(written.len(), 2);
        assert!(dest.join("sub").is_dir());
        assert_eq!(std::fs::read(dest.join("sub").join("a.txt")).unwrap(), b"child");
        // The existing file survives under its name; the archive copy is renamed.
        assert_eq!(std::fs::read(dest.join("top.txt")).unwrap(), b"keep");
        assert_eq!(std::fs::read(dest.join("top (1).txt")).unwrap(), b"top");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn volume_families_group_only_their_own_set() {
        let part = VolumeFamily::of("拍 摄 集.part1.rar").unwrap();
        assert_eq!(part.key("拍 摄 集.part1.rar"), Some(1));
        assert_eq!(part.key("拍 摄 集.part02.RAR"), Some(2));
        assert_eq!(part.key("拍 摄 集.part10.rar"), Some(10));
        assert_eq!(part.key("拍 摄 集.zip"), None);
        assert_eq!(part.key("别的.part2.rar"), None);

        let legacy = VolumeFamily::of("数据.r00").unwrap();
        assert_eq!(legacy.key("数据.rar"), Some(0));
        assert_eq!(legacy.key("数据.r00"), Some(1));
        assert_eq!(legacy.key("数据.R12"), Some(13));
        assert_eq!(legacy.key("数据.txt"), None);

        let numbered = VolumeFamily::of("视频.001").unwrap();
        assert_eq!(numbered.key("视频.001"), Some(1));
        assert_eq!(numbered.key("视频.007"), Some(7));
        assert_eq!(numbered.key("视频.001.001"), None);

        assert!(VolumeFamily::of("单卷.rar").is_some());
        assert!(VolumeFamily::of("模糊文件").is_none());
    }

    #[test]
    fn volume_key_does_not_slice_inside_a_character() {
        // `İ` is two bytes and `中` starts with a different two-byte sequence; the prefix
        // from the first name must not be used as a byte offset into the second.
        let part = VolumeFamily::of("İ.part1.rar").unwrap();
        assert_eq!(part.key("中文.rar"), None);
        assert_eq!(part.key("İ.part2.rar"), Some(2));
        assert_eq!(part.key("İ.part02.rar"), Some(2));
    }

    #[test]
    fn listing_reports_when_the_cap_hid_entries() {
        let dir = std::env::temp_dir().join(format!("flowdir-list-trunc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let zip_path = dir.join("tree.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for name in ["a.txt", "b.txt", "c.txt"] {
            writer.start_file(name, options).unwrap();
            writer.write_all(b"x").unwrap();
        }
        writer.finish().unwrap();

        let tar_path = dir.join("tree.tar");
        let mut builder = tar::Builder::new(std::fs::File::create(&tar_path).unwrap());
        for name in ["a.txt", "b.txt", "c.txt"] {
            let mut header = tar::Header::new_gnu();
            header.set_size(1);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, &b"x"[..]).unwrap();
        }
        builder.finish().unwrap();

        for archive in [&zip_path, &tar_path] {
            let path = archive.to_string_lossy().into_owned();
            let (entries, truncated) = list_with_truncated(&path, 2).unwrap();
            assert_eq!(entries.len(), 2, "{}", path);
            assert!(truncated, "{}", path);
            // Exactly the member count is not truncation; one less is.
            let (entries, truncated) = list_with_truncated(&path, 3).unwrap();
            assert_eq!(entries.len(), 3, "{}", path);
            assert!(!truncated, "{}", path);
            assert_eq!(list(&path, 2).unwrap().len(), 2, "{}", path);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tar_inner_dir_scan_matches_directories_only() {
        let dir = std::env::temp_dir().join(format!("flowdir-tar-inner-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let tar_path = dir.join("tree.tar");
        let mut builder = tar::Builder::new(std::fs::File::create(&tar_path).unwrap());
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o755);
        header.set_entry_type(tar::EntryType::Directory);
        header.set_cksum();
        builder.append_data(&mut header, "docs/", &[][..]).unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_size(1);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, "docs/a.txt", &b"x"[..]).unwrap();
        // A file whose name matches a queried path must not be reported as a directory.
        let mut header = tar::Header::new_gnu();
        header.set_size(1);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, "notes.txt", &b"y"[..]).unwrap();
        builder.finish().unwrap();

        let zip_path = dir.join("tree.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        zip.start_file("docs/a.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"x").unwrap();
        zip.finish().unwrap();

        let tar = tar_path.to_string_lossy().into_owned();
        assert_eq!(tar_contains_entry(&tar, "docs", 100).unwrap(), Some(true));
        assert_eq!(tar_contains_entry(&tar, "missing", 100).unwrap(), Some(false));
        assert_eq!(
            tar_contains_entry(&tar, "notes.txt", 100).unwrap(),
            Some(false)
        );
        let zip = zip_path.to_string_lossy().into_owned();
        assert_eq!(tar_contains_entry(&zip, "docs", 100).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncated_tar_listing_is_an_error_not_a_partial_success() {
        let dir = std::env::temp_dir().join(format!("flowdir-tar-cut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let tar_path = dir.join("cut.tar");
        let mut builder = tar::Builder::new(std::fs::File::create(&tar_path).unwrap());
        for name in ["big.txt", "after.txt"] {
            let body = vec![b'x'; 8192];
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, &body[..]).unwrap();
        }
        builder.finish().unwrap();

        // Keep the first header and half of its payload: reaching the second entry has to
        // skip the missing bytes and must surface the error.
        let bytes = std::fs::read(&tar_path).unwrap();
        std::fs::write(&tar_path, &bytes[..512 + 4096]).unwrap();
        let path = tar_path.to_string_lossy().into_owned();
        assert!(list_unbounded_with_truncated(&path, 100).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
