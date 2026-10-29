//! Extraction backends: one reader per format family, all of them feeding the same sink.
//!
//! Every backend only decodes; naming, limits, progress, cancellation and the move into
//! the destination live in [`crate::archive::sink`].

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path as FsPath, PathBuf};

use crate::error::ApiError;

use super::entry;
use super::format::{self, Format};
use super::sink::{Sink, Take};
use super::ArchiveEntry;

/// Decompressed bytes a preview may read before it reports what it has so far.
const PREVIEW_BYTES: u64 = 64 * 1024 * 1024;

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
            Err(err) => {
                sink.skip(&file.name, &err.code);
                return Ok(true);
            }
        };
        match sink.accept(&rel, file.is_directory, false) {
            Ok(Take::Skip | Take::Dir) => Ok(true),
            Ok(Take::File(target)) => match sink.write(&target, read, file.size) {
                Ok(()) => Ok(true),
                Err(err) => {
                    abort = Some(err);
                    Ok(false)
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
                if ext != "rar" || stem.len() < prefix.len() {
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
        // checksum; without a password the same failure means real corruption.
        RarError::DataCrcMismatch { .. }
        | RarError::PackedDataCrcMismatch { .. }
        | RarError::Blake2Mismatch { .. }
        | RarError::PackedDataBlake2Mismatch { .. }
            if had_password =>
        {
            ApiError::new("WRONG_PASSWORD", "wrong password")
        }
        other => ApiError::new("ARCHIVE", other.to_string()),
    }
}

/// ISO/UDF images are parsed into a tree first: the format has no random access cost and
/// the tree gives both the totals and the extraction order.
fn extract_iso(path: &str, sink: &mut Sink) -> Result<(), ApiError> {
    let mut file = File::open(path).map_err(|err| ApiError::from_io(path, &err))?;
    let root = isomage::detect_and_parse_filesystem(&mut file, name_of(path))
        .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
    let (entries, bytes) = iso_totals(&root);
    sink.set_totals(Some(entries), Some(bytes))?;
    sink.progress_phase("extract");
    extract_iso_dir(&mut file, root, "", sink)
}

fn iso_totals(node: &isomage::TreeNode) -> (u32, u64) {
    let mut entries = 0u32;
    let mut bytes = 0u64;
    for child in &node.children {
        entries += 1;
        if child.is_directory {
            let (nested_entries, nested_bytes) = iso_totals(child);
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
) -> Result<(), ApiError> {
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
            Err(err) => {
                sink.skip(&raw, &err.code);
                continue;
            }
        };
        if child.is_directory {
            sink.accept(&rel, true, false)?;
            extract_iso_dir(file, child.clone(), &raw, sink)?;
        } else if let Take::File(target) = sink.accept(&rel, false, false)? {
            let size = child.size;
            sink.write_with(&target, size, |writer| {
                isomage::cat_node(file, child, writer)
                    .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))
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
        let rel = match entry::decode(name.as_bytes(), utf8) {
            Ok(rel) => rel,
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
            Err(_) if password.is_some() => {
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
        // A damaged tail should not discard what already came out of the stream.
        let Ok(mut entry) = entry else { break };
        let header = entry.header();
        let kind = header.entry_type();
        let is_dir = kind.is_dir();
        let link = kind.is_symlink() || kind.is_hard_link();
        let declared = header.size().unwrap_or(0);
        let raw = entry.path_bytes().to_vec();
        let rel = match entry::decode(&raw, false) {
            Ok(rel) => rel,
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
pub fn list(path: &str, limit: usize) -> Result<Vec<ArchiveEntry>, ApiError> {
    let format = format::detect(path).ok_or_else(|| ApiError::new("UNSUPPORTED_FORMAT", path))?;
    if format == Format::Zip {
        let file = File::open(path).map_err(|e| ApiError::from_io(path, &e))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| ApiError::new("ARCHIVE", e.to_string()))?;
        let mut entries = Vec::new();
        for index in 0..archive.len().min(limit) {
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
        return Ok(entries);
    }
    if format == Format::SevenZ {
        use sevenz_rust2::{ArchiveReader, Password};
        let reader = ArchiveReader::open(path, Password::empty())
            .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
        return Ok(reader
            .archive()
            .files
            .iter()
            .filter(|file| !file.name.is_empty())
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
            .collect());
    }
    if format == Format::Cab {
        let file = File::open(path).map_err(|err| ApiError::from_io(path, &err))?;
        let cabinet =
            cab::Cabinet::new(file).map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
        let mut entries = Vec::new();
        for folder in cabinet.folder_entries() {
            for item in folder.file_entries() {
                if entries.len() >= limit {
                    return Ok(entries);
                }
                if let Ok(name) = entry::decode(item.name().as_bytes(), item.is_name_utf()) {
                    entries.push(ArchiveEntry {
                        name,
                        size: item.uncompressed_size() as u64,
                        is_dir: false,
                        mtime: 0,
                    });
                }
            }
        }
        return Ok(entries);
    }
    if format == Format::Rar {
        let archive = open_rar(path, None)?;
        let mut entries = Vec::new();
        for member in archive.entries() {
            if entries.len() >= limit {
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
        return Ok(entries);
    }
    if format == Format::Iso {
        let mut file = File::open(path).map_err(|err| ApiError::from_io(path, &err))?;
        let root = isomage::detect_and_parse_filesystem(&mut file, name_of(path))
            .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
        let mut entries = Vec::new();
        push_iso_entries(&root, "", limit, &mut entries);
        return Ok(entries);
    }
    if format.is_tar() {
        let mut entries = Vec::new();
        // Listing a compressed tar means decompressing it; stop after a bounded amount so
        // previewing a multi-gigabyte archive cannot stall the panel.
        let mut archive = tar::Archive::new(reader(path, format)?.take(PREVIEW_BYTES));
        let Ok(items) = archive.entries() else {
            return Ok(entries);
        };
        for item in items.take(limit) {
            let Ok(item) = item else { break };
            let raw = item.path_bytes().to_vec();
            let Ok(name) = entry::decode(&raw, false) else {
                continue;
            };
            let is_dir = item.header().entry_type().is_dir();
            let mtime = item.header().mtime().unwrap_or(0) * 1000;
            entries.push(ArchiveEntry {
                name,
                size: if is_dir { 0 } else { item.header().size().unwrap_or(0) },
                is_dir,
                mtime,
            });
        }
        return Ok(entries);
    }
    Ok(vec![ArchiveEntry {
        name: format::output_name(name_of(path)),
        size: 0,
        is_dir: false,
        mtime: 0,
    }])
}

fn push_iso_entries(node: &isomage::TreeNode, prefix: &str, limit: usize, out: &mut Vec<ArchiveEntry>) {
    for child in &node.children {
        if out.len() >= limit {
            return;
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
        if child.is_directory {
            push_iso_entries(child, &raw, limit, out);
        }
    }
}

/// Extracts a single entry from `archive` into `dest`.
pub fn extract_single(
    archive: &str,
    inner: &str,
    dest: &std::path::Path,
    password: Option<&str>,
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
                        let mut out = File::create(dest).map_err(|e| ApiError::from_io(&dest.to_string_lossy(), &e))?;
                        std::io::copy(&mut member, &mut out).map_err(|e| ApiError::from_io(&dest.to_string_lossy(), &e))?;
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
                        match File::create(dest) {
                            Ok(mut out) => {
                                if let Err(e) = std::io::copy(read, &mut out) {
                                    copy_err = Some(ApiError::from_io(&dest.to_string_lossy(), &e));
                                }
                            }
                            Err(e) => {
                                copy_err = Some(ApiError::from_io(&dest.to_string_lossy(), &e));
                            }
                        }
                        return Ok(false);
                    }
                }
                Ok(true)
            });
            if let Some(err) = copy_err {
                return Err(err);
            }
            if found {
                return Ok(());
            }
            Err(ApiError::new("NOT_FOUND", inner))
        }
        format if format.is_tar() => {
            let mut tar = tar::Archive::new(reader(archive, format)?);
            let items = tar.entries().map_err(|e| ApiError::new("ARCHIVE", e.to_string()))?;
            for item in items {
                let Ok(mut item) = item else { continue; };
                let raw = item.path_bytes().to_vec();
                if let Ok(name) = entry::decode(&raw, false) {
                    if name.eq_ignore_ascii_case(inner_norm) {
                        let mut out = File::create(dest).map_err(|e| ApiError::from_io(&dest.to_string_lossy(), &e))?;
                        std::io::copy(&mut item, &mut out).map_err(|e| ApiError::from_io(&dest.to_string_lossy(), &e))?;
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
                    if let Ok(name) = entry::decode(item.name().as_bytes(), item.is_name_utf()) {
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
                let mut out = File::create(dest).map_err(|e| ApiError::from_io(&dest.to_string_lossy(), &e))?;
                std::io::copy(&mut reader, &mut out).map_err(|e| ApiError::from_io(&dest.to_string_lossy(), &e))?;
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
                        let mut out = File::create(dest).map_err(|e| ApiError::from_io(&dest.to_string_lossy(), &e))?;
                        let had_password = password.is_some();
                        entry.copy_to(&mut out).map_err(|err| rar_error(err, had_password))?;
                        return Ok(());
                    }
                }
            }
            Err(ApiError::new("NOT_FOUND", inner))
        }
        Format::Iso => {
            let mut file = File::open(archive).map_err(|err| ApiError::from_io(archive, &err))?;
            let root = isomage::detect_and_parse_filesystem(&mut file, name_of(archive))
                .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
            if extract_iso_single(&mut file, &root, "", inner_norm, dest)? {
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
) -> Result<bool, ApiError> {
    for child in &node.children {
        let raw = if prefix.is_empty() {
            child.name.clone()
        } else {
            format!("{prefix}/{}", child.name)
        };
        if let Ok(name) = entry::decode(raw.as_bytes(), true) {
            if child.is_directory {
                if extract_iso_single(file, child, &raw, target, dest)? {
                    return Ok(true);
                }
            } else if name.eq_ignore_ascii_case(target) {
                let mut out = File::create(dest).map_err(|e| ApiError::from_io(&dest.to_string_lossy(), &e))?;
                isomage::cat_node(file, child, &mut out).map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
                return Ok(true);
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
    use super::VolumeFamily;

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
}
