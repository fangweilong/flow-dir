//! Extraction backends: one reader per format family, all of them feeding the same sink.
//!
//! Every backend only decodes; naming, limits, progress, cancellation and the move into
//! the destination live in [`crate::archive::sink`].

use std::fs::File;
use std::io::{BufReader, Read};
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
        Format::Zip | Format::SevenZ | Format::Cab => {
            return Err(ApiError::new("INTERNAL", "not a stream format"))
        }
    };
    Ok(reader)
}

pub fn extract(archive: &str, sink: &mut Sink) -> Result<(), ApiError> {
    let format = format::detect(archive)
        .ok_or_else(|| ApiError::new("UNSUPPORTED_FORMAT", name_of(archive)))?;
    match format {
        // Indexed formats know their totals up front; show that pass as its own phase so
        // a huge archive does not look idle.
        Format::Zip => {
            sink.progress_phase("plan");
            extract_zip(archive, sink)
        }
        Format::SevenZ => {
            sink.progress_phase("plan");
            extract_7z(archive, sink)
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

fn extract_7z(path: &str, sink: &mut Sink) -> Result<(), ApiError> {
    use sevenz_rust2::{ArchiveReader, Password};

    let mut reader = ArchiveReader::open(path, Password::empty())
        .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
    let entries: Vec<(String, u64, bool)> = reader
        .archive()
        .files
        .iter()
        .map(|file| (file.name.clone(), file.size, file.is_directory))
        .collect();
    let total: u64 = entries.iter().filter(|(_, _, dir)| !dir).map(|(_, size, _)| *size).sum();
    sink.set_totals(Some(entries.len() as u32), Some(total))?;
    sink.progress_phase("extract");

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
    walked.map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
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

fn extract_zip(path: &str, sink: &mut Sink) -> Result<(), ApiError> {
    let file = File::open(path).map_err(|e| ApiError::from_io(path, &e))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| ApiError::new("ARCHIVE", e.to_string()))?;
    let mut total = 0u64;
    for index in 0..archive.len() {
        if let Ok(entry) = archive.by_index(index) {
            total += entry.size();
        }
    }
    sink.set_totals(Some(archive.len() as u32), Some(total))?;
    sink.progress_phase("extract");

    let mut encrypted = 0u32;
    let mut written = 0u32;
    for index in 0..archive.len() {
        if sink.cancelled() {
            return Err(ApiError::new("CANCELED", "extraction canceled"));
        }
        let mut file = match archive.by_index(index) {
            Ok(file) => file,
            Err(err) => {
                sink.skip(&format!("entry #{index}"), "CORRUPT");
                let _ = err;
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
        if file.encrypted() {
            encrypted += 1;
            sink.skip(&rel, "ENCRYPTED");
            continue;
        }
        let declared = file.size();
        let is_dir = file.is_dir();
        let link = file.is_symlink();
        match sink.accept(&rel, is_dir, link)? {
            Take::Skip | Take::Dir => {}
            Take::File(target) => {
                sink.write(&target, &mut file, declared)?;
                written += 1;
            }
        }
    }
    if written == 0 && encrypted > 0 {
        return Err(ApiError::new(
            "ENCRYPTED",
            "password-protected archives are not supported yet",
        ));
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
            if let Ok(name) = entry::decode(raw, utf8) {
                entries.push(ArchiveEntry {
                    name,
                    size: file.size(),
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
                entry::decode(file.name.as_bytes(), true)
                    .ok()
                    .map(|name| ArchiveEntry {
                        name,
                        size: file.size,
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
                    });
                }
            }
        }
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
            entries.push(ArchiveEntry {
                name,
                size: item.header().size().unwrap_or(0),
            });
        }
        return Ok(entries);
    }
    Ok(vec![ArchiveEntry {
        name: format::output_name(name_of(path)),
        size: 0,
    }])
}

fn name_of(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}
