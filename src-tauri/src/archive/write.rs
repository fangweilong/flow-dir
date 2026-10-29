//! Archive creation: zip, 7z and the compressed tar family.
//!
//! Writing is a separate pipeline from extraction. Sources are walked once into a
//! deterministic, sorted list, then one backend streams them into a temporary file next to
//! the destination; the temporary is renamed only after the whole archive is complete, so a
//! cancel or a mid-way failure never leaves a half-written archive under the user's name.

use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tauri::ipc::Channel;

use crate::error::ApiError;
use crate::ops::{OpProgress, OpState};

/// Copy size per read: large enough for syscall overhead to disappear, small enough that
/// cancel and progress stay responsive on a slow disk.
const CHUNK: usize = 256 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(80);
const MAX_ENTRIES: usize = 200_000;

/// Output formats this module can write. RAR is missing on purpose: the UnRAR license
/// forbids shipping a compressor, and ISO needs a writer crate that is not in the tree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Zip,
    SevenZ,
    TarGz,
    TarXz,
    TarZst,
}

impl Format {
    fn parse(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "zip" => Self::Zip,
            "7z" => Self::SevenZ,
            "tar.gz" => Self::TarGz,
            "tar.xz" => Self::TarXz,
            "tar.zst" => Self::TarZst,
            _ => return None,
        })
    }

    /// 7z encryption is unavailable: sevenz-rust2 writes plain archives only.
    fn encrypts(self) -> bool {
        self == Self::Zip
    }
}

/// One request as it arrives from the UI; ids are strings so the frontend owns the mapping.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub job_id: String,
    pub paths: Vec<String>,
    pub dest: String,
    /// `zip` | `7z` | `tar.gz` | `tar.xz` | `tar.zst`.
    pub format: String,
    /// 0-9, clamped; 0 stores without compressing.
    #[serde(default)]
    pub level: u32,
    pub password: Option<String>,
}

/// A file or directory to store, with the name it gets inside the archive.
struct Source {
    disk: PathBuf,
    name: String,
    size: u64,
    dir: bool,
}

struct Progress {
    job_id: String,
    channel: Channel<OpProgress>,
    entries: u32,
    entries_total: u32,
    bytes: u64,
    bytes_total: u64,
    current: Option<String>,
    last: Instant,
}

impl Progress {
    fn report(&mut self, done: bool) -> OpProgress {
        OpProgress {
            op_id: self.job_id.clone(),
            phase: "compress".into(),
            current: self.entries,
            total: self.entries_total,
            name: self.current.clone(),
            work: self.bytes,
            work_total: self.bytes_total,
            failed_count: 0,
            failed: Vec::new(),
            done,
            aborted: false,
            error: None,
        }
    }

    fn tick(&mut self) {
        if self.last.elapsed() >= PROGRESS_INTERVAL {
            self.last = Instant::now();
            let progress = self.report(false);
            let _ = self.channel.send(progress);
        }
    }
}

/// Starts a compression job on a background thread. Every precondition is checked here so
/// the command can report it as a normal error instead of a failed job.
pub fn start(
    state: &OpState,
    request: Request,
    channel: Channel<OpProgress>,
) -> Result<(), ApiError> {
    let format = Format::parse(&request.format)
        .ok_or_else(|| ApiError::new("UNSUPPORTED_FORMAT", request.format.clone()))?;
    if request.paths.is_empty() {
        return Err(ApiError::new("EMPTY", "no paths"));
    }
    for path in &request.paths {
        if !Path::new(path).exists() {
            return Err(ApiError::new("NOT_FOUND", path));
        }
    }
    let dest = PathBuf::from(&request.dest);
    let parent = dest
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| ApiError::new("INVALID_DEST", request.dest.clone()))?;
    if !parent.is_dir() {
        return Err(ApiError::new("INVALID_DEST", request.dest.clone()));
    }
    if request.password.is_some() && !format.encrypts() {
        return Err(ApiError::new("PASSWORD_UNSUPPORTED", request.format.clone()));
    }
    let level = request.level.min(9);

    let token = state.begin_external(&request.job_id);
    let state = state.clone();
    std::thread::Builder::new()
        .name(format!("flowdir-compress-{}", request.job_id))
        .spawn(move || {
            let job_id = request.job_id.clone();
            let _ = run(request, format, level, token, channel);
            state.finish_external(&job_id);
        })
        .map(|_| ())
        .map_err(|err| ApiError::new("THREAD", err.to_string()))
}

/// Runs one job to completion, emitting progress and one final message.
fn run(
    request: Request,
    format: Format,
    level: u32,
    cancel: Arc<AtomicBool>,
    channel: Channel<OpProgress>,
) -> OpProgress {
    let dest = PathBuf::from(&request.dest);
    log::info!(
        target: "flowdir::archive",
        "compress {} entries -> {} ({:?})",
        request.paths.len(),
        request.dest,
        format
    );
    let tmp = temporary(&dest);
    let result: Result<Progress, ApiError> = (|| {
        let sources = collect(&request.paths, &dest, &tmp)?;
        let bytes_total = sources.iter().map(|source| source.size).sum();
        let mut progress = Progress {
            job_id: request.job_id.clone(),
            channel: channel.clone(),
            entries: 0,
            entries_total: sources.len().min(u32::MAX as usize) as u32,
            bytes: 0,
            bytes_total,
            current: None,
            last: Instant::now(),
        };
        write_archive(format, &tmp, &sources, level, request.password.as_deref(), &mut progress, &cancel)?;
        fs::rename(&tmp, &dest).map_err(|err| ApiError::from_io(&request.dest, &err))?;
        Ok(progress)
    })();

    let progress = match result {
        Ok(mut progress) => {
            progress.current = None;
            log::info!(
                target: "flowdir::archive",
                "compress {} done: {} entries, {} bytes in",
                request.dest, progress.entries, progress.bytes
            );
            progress.report(true)
        }
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            let aborted = err.code == "CANCELED";
            if aborted {
                log::warn!(target: "flowdir::archive", "compress {} canceled", request.dest);
            } else {
                log::error!(
                    target: "flowdir::archive",
                    "compress {} failed: {} {}",
                    request.dest, err.code, err.detail
                );
            }
            OpProgress {
                op_id: request.job_id,
                phase: "done".into(),
                current: 0,
                total: 0,
                name: None,
                work: 0,
                work_total: 0,
                failed_count: 0,
                failed: Vec::new(),
                done: true,
                aborted,
                error: if aborted { None } else { Some(err) },
            }
        }
    };
    let _ = channel.send(progress.clone());
    progress
}

/// `<dest>.part`: same directory, so the final rename stays on one volume.
fn temporary(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// Walks the selection once. Names are relative to the parent of each selected item, so a
/// selected folder lands under its own name and selected files land at the archive root.
fn collect(paths: &[String], dest: &Path, tmp: &Path) -> Result<Vec<Source>, ApiError> {
    let skip = [normal(dest), normal(tmp)];
    let mut sources = Vec::new();
    let mut seen_dirs = Vec::new();
    let mut roots: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    roots.sort();
    for root in roots {
        let meta = fs::metadata(&root).map_err(|err| ApiError::from_io(&root.display().to_string(), &err))?;
        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| ApiError::new("INVALID_NAME", root.display().to_string()))?;
        let mut stack = vec![(root.clone(), name, meta)];
        while let Some((disk, name, meta)) = stack.pop() {
            if meta.is_dir() {
                let canonical = normal(&disk);
                if seen_dirs.contains(&canonical) {
                    continue;
                }
                seen_dirs.push(canonical);
                sources.push(Source {
                    disk: disk.clone(),
                    name: name.clone(),
                    size: 0,
                    dir: true,
                });
                let mut children: Vec<(PathBuf, String, fs::Metadata)> = Vec::new();
                let entries = fs::read_dir(&disk)
                    .map_err(|err| ApiError::from_io(&disk.display().to_string(), &err))?;
                for entry in entries {
                    let entry = entry.map_err(|err| ApiError::from_io(&disk.display().to_string(), &err))?;
                    let child = entry.path();
                    if skip.contains(&normal(&child)) {
                        continue;
                    }
                    let meta = fs::metadata(&child)
                        .map_err(|err| ApiError::from_io(&child.display().to_string(), &err))?;
                    let child_name = format!("{}/{}", name, entry.file_name().to_string_lossy());
                    children.push((child, child_name, meta));
                }
                children.sort_by(|a, b| b.1.cmp(&a.1));
                stack.extend(children);
            } else {
                sources.push(Source {
                    disk,
                    name,
                    size: meta.len(),
                    dir: false,
                });
            }
        }
    }
    if sources.is_empty() {
        return Err(ApiError::new("EMPTY", "nothing to compress"));
    }
    if sources.len() > MAX_ENTRIES {
        return Err(ApiError::new("TOO_MANY_ENTRIES", sources.len().to_string()));
    }
    Ok(sources)
}

/// Comparison key for paths that may not exist yet (the temporary file).
fn normal(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Copies one entry's bytes, stopping on cancel and reporting progress as it goes.
fn pump(
    reader: &mut impl Read,
    writer: &mut impl Write,
    size: u64,
    progress: &mut Progress,
    cancel: &AtomicBool,
    name: &str,
) -> Result<(), ApiError> {
    let mut buffer = vec![0u8; CHUNK.min(size.max(1) as usize)];
    let mut copied = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ApiError::new("CANCELED", name));
        }
        let read = reader
            .read(&mut buffer)
            .map_err(|err| ApiError::from_io(name, &err))?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|err| ApiError::new("WRITE", format!("{name}: {err}")))?;
        copied += read as u64;
        progress.bytes += read as u64;
        progress.tick();
    }
    if copied != size {
        return Err(ApiError::new(
            "CHANGED",
            format!("{name}: expected {size} bytes, read {copied}"),
        ));
    }
    Ok(())
}

fn write_archive(
    format: Format,
    tmp: &Path,
    sources: &[Source],
    level: u32,
    password: Option<&str>,
    progress: &mut Progress,
    cancel: &AtomicBool,
) -> Result<(), ApiError> {
    match format {
        Format::Zip => write_zip(tmp, sources, level, password, progress, cancel),
        Format::SevenZ => write_sevenz(tmp, sources, level, progress, cancel),
        Format::TarGz | Format::TarXz | Format::TarZst => {
            write_tar(tmp, format, sources, level, progress, cancel)
        }
    }
}

fn write_zip(
    tmp: &Path,
    sources: &[Source],
    level: u32,
    password: Option<&str>,
    progress: &mut Progress,
    cancel: &AtomicBool,
) -> Result<(), ApiError> {
    use zip::write::SimpleFileOptions;
    use zip::{AesMode, CompressionMethod, ZipWriter};

    let file = File::create(tmp).map_err(|err| ApiError::from_io(&tmp.display().to_string(), &err))?;
    let mut zip = ZipWriter::new(BufWriter::new(file));
    let options = |method: CompressionMethod, large: bool| {
        let mut options = SimpleFileOptions::default()
            .compression_method(method)
            .compression_level(Some(i64::from(level)));
        if large {
            options = options.large_file(true);
        }
        if let Some(password) = password {
            options = options.with_aes_encryption(AesMode::Aes256, password);
        }
        options
    };
    for source in sources {
        if cancel.load(Ordering::Relaxed) {
            return Err(ApiError::new("CANCELED", source.name.as_str()));
        }
        progress.current = Some(source.name.clone());
        if source.dir {
            zip.add_directory(format!("{}/", source.name), options(CompressionMethod::Stored, false))
                .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
            progress.entries += 1;
            progress.tick();
            continue;
        }
        let large = source.size > u64::from(u32::MAX);
        // Level 0 means "store": deflate at level 0 still carries block overhead.
        let method = if level == 0 {
            CompressionMethod::Stored
        } else {
            CompressionMethod::Deflated
        };
        zip.start_file(&source.name, options(method, large))
            .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
        let mut file = File::open(&source.disk)
            .map_err(|err| ApiError::from_io(&source.disk.display().to_string(), &err))?;
        pump(&mut file, &mut zip, source.size, progress, cancel, &source.name)?;
        progress.entries += 1;
    }
    let mut file = zip
        .finish()
        .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
    file.flush().map_err(|err| ApiError::new("WRITE", err.to_string()))?;
    Ok(())
}

fn write_sevenz(
    tmp: &Path,
    sources: &[Source],
    level: u32,
    progress: &mut Progress,
    cancel: &AtomicBool,
) -> Result<(), ApiError> {
    use sevenz_rust2::encoder_options::{EncoderOptions, Lzma2Options};
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter, EncoderConfiguration, EncoderMethod};

    let mut writer = ArchiveWriter::create(tmp).map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
    let method = if level == 0 {
        EncoderConfiguration::new(EncoderMethod::COPY)
    } else {
        EncoderConfiguration::new(EncoderMethod::LZMA2)
            .with_options(EncoderOptions::Lzma2(Lzma2Options::from_level(level)))
    };
    writer.set_content_methods(vec![method]).set_encrypt_header(false);
    for source in sources {
        if cancel.load(Ordering::Relaxed) {
            return Err(ApiError::new("CANCELED", source.name.as_str()));
        }
        progress.current = Some(source.name.clone());
        let entry = if source.dir {
            ArchiveEntry::new_directory(&source.name)
        } else {
            ArchiveEntry::from_path(&source.disk, source.name.clone())
        };
        if source.dir {
            writer
                .push_archive_entry::<io::Empty>(entry, None)
                .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
        } else {
            let file = File::open(&source.disk)
                .map_err(|err| ApiError::from_io(&source.disk.display().to_string(), &err))?;
            let mut limited = Counted::new(file, source.size, cancel, progress)?;
            writer
                .push_archive_entry(entry, Some(&mut limited))
                .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
            limited.finish(&source.name)?;
        }
        progress.entries += 1;
        progress.tick();
    }
    writer.finish().map_err(|err| ApiError::new("WRITE", err.to_string()))?;
    Ok(())
}

fn write_tar(
    tmp: &Path,
    format: Format,
    sources: &[Source],
    level: u32,
    progress: &mut Progress,
    cancel: &AtomicBool,
) -> Result<(), ApiError> {
    let file = File::create(tmp).map_err(|err| ApiError::from_io(&tmp.display().to_string(), &err))?;
    let writer = BufWriter::new(file);
    let encoder: Box<dyn Write> = match format {
        Format::TarGz => Box::new(flate2::write::GzEncoder::new(
            writer,
            flate2::Compression::new(level),
        )),
        Format::TarXz => {
            let mut options = lzma_rust2::XzOptions::with_preset(level);
            options.set_block_size(std::num::NonZeroU64::new(64 << 20));
            Box::new(
                lzma_rust2::XzWriter::new(writer, options)
                    .map_err(|err| ApiError::new("WRITE", err.to_string()))?,
            )
        }
        _ => Box::new(
            zstd::Encoder::new(writer, level as i32)
                .map_err(|err| ApiError::new("WRITE", err.to_string()))?
                .auto_finish(),
        ),
    };
    let mut builder = tar::Builder::new(encoder);
    builder.mode(tar::HeaderMode::Complete);
    for source in sources {
        if cancel.load(Ordering::Relaxed) {
            return Err(ApiError::new("CANCELED", source.name.as_str()));
        }
        progress.current = Some(source.name.clone());
        let mut header = tar::Header::new_gnu();
        if source.dir {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_mode(0o755);
            builder
                .append_data(&mut header, format!("{}/", source.name), io::empty())
                .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
            progress.entries += 1;
            progress.tick();
            continue;
        }
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(source.size);
        header.set_mode(0o644);
        // `append_data` seeks back to fix the checksum, so the payload must be writable
        // through the same entry: the tar crate does that when the reader is sized.
        let file = File::open(&source.disk)
            .map_err(|err| ApiError::from_io(&source.disk.display().to_string(), &err))?;
        let mut limited = Counted::new(file, source.size, cancel, progress)?;
        builder
            .append_data(&mut header, &source.name, &mut limited)
            .map_err(|err| ApiError::new("ARCHIVE", err.to_string()))?;
        limited.finish(&source.name)?;
        progress.entries += 1;
    }
    let encoder = builder
        .into_inner()
        .map_err(|err| ApiError::new("WRITE", err.to_string()))?;
    let mut encoder = encoder;
    encoder.flush().map_err(|err| ApiError::new("WRITE", err.to_string()))?;
    drop(encoder);
    Ok(())
}

/// Reader that stops at the declared size (a file that grew or shrank is an error) and
/// feeds the job's progress and cancel token while tar pulls bytes.
struct Counted<'a, R> {
    inner: R,
    remaining: u64,
    cancel: &'a AtomicBool,
    progress: &'a mut Progress,
}

impl<'a, R: Read> Counted<'a, R> {
    fn new(
        inner: R,
        size: u64,
        cancel: &'a AtomicBool,
        progress: &'a mut Progress,
    ) -> Result<Self, ApiError> {
        Ok(Self {
            inner,
            remaining: size,
            cancel,
            progress,
        })
    }

    fn finish(&self, name: &str) -> Result<(), ApiError> {
        if self.remaining != 0 {
            return Err(ApiError::new(
                "CHANGED",
                format!("{name}: expected payload ended early"),
            ));
        }
        Ok(())
    }
}

impl<R: Read> Read for Counted<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "canceled"));
        }
        let limit = buffer.len().min(self.remaining as usize).max(1);
        let read = self.inner.read(&mut buffer[..limit])?;
        if read == 0 && self.remaining > 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "file shrank while compressing",
            ));
        }
        self.remaining -= read as u64;
        self.progress.bytes += read as u64;
        self.progress.tick();
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flowdir-compress-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn channel() -> Channel<OpProgress> {
        Channel::new(|_| Ok(()))
    }

    /// Names and payloads of an archive, read back with the production extractor.
    fn round_trip(archive: &Path, dest: &Path, password: Option<&str>) -> OpProgress {
        super::super::run(
            "read-back".into(),
            archive.to_string_lossy().into_owned(),
            dest.to_string_lossy().into_owned(),
            password.map(str::to_string),
            Arc::new(AtomicBool::new(false)),
            channel(),
        )
    }

    fn compress(
        paths: &[String],
        dest: &Path,
        format: Format,
        level: u32,
        password: Option<&str>,
    ) -> OpProgress {
        let request = Request {
            job_id: "test-compress".into(),
            paths: paths.to_vec(),
            dest: dest.to_string_lossy().into_owned(),
            format: match format {
                Format::Zip => "zip",
                Format::SevenZ => "7z",
                Format::TarGz => "tar.gz",
                Format::TarXz => "tar.xz",
                Format::TarZst => "tar.zst",
            }
            .into(),
            level,
            password: password.map(str::to_string),
        };
        run(request, format, level, Arc::new(AtomicBool::new(false)), channel())
    }

    /// Folder with a nested file, an empty directory and non-ASCII names.
    fn source(name: &str) -> PathBuf {
        let root = temp_dir(name);
        fs::create_dir_all(root.join("资料/空目录")).unwrap();
        fs::write(root.join("a.txt"), b"alpha").unwrap();
        fs::write(root.join("资料/说明.txt"), "中文内容".as_bytes()).unwrap();
        root
    }

    const FORMATS: [(&str, Format, &str); 5] = [
        ("zip", Format::Zip, ".zip"),
        ("sevenz", Format::SevenZ, ".7z"),
        ("targz", Format::TarGz, ".tar.gz"),
        ("tarxz", Format::TarXz, ".tar.xz"),
        ("tarzst", Format::TarZst, ".tar.zst"),
    ];

    #[test]
    fn every_format_round_trips_the_same_tree() {
        for (label, format, extension) in FORMATS {
            let src = source(&format!("round-{label}"));
            let dest = src.with_file_name(format!("{label}{extension}"));
            let progress = compress(&[src.to_string_lossy().into_owned()], &dest, format, 5, None);
            assert!(progress.error.is_none(), "{label}: {:?}", progress.error);
            assert_eq!(progress.aborted, false);
            assert!(dest.is_file(), "{label} must produce {extension}");

            let out = temp_dir(&format!("out-{label}"));
            let read = round_trip(&dest, &out, None);
            assert!(read.error.is_none(), "{label}: {:?}", read.error);
            let root = out.join(src.file_name().unwrap());
            assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"alpha", "{label}");
            assert_eq!(
                fs::read_to_string(root.join("资料/说明.txt")).unwrap(),
                "中文内容",
                "{label}"
            );
            assert!(root.join("资料/空目录").is_dir(), "{label} must keep the empty dir");
            let _ = fs::remove_dir_all(&src);
            let _ = fs::remove_dir_all(&out);
        }
    }

    #[test]
    fn zip_password_is_required_to_read_back() {
        let src = source("zip-pw");
        let dest = src.with_file_name("locked.zip");
        let progress = compress(
            &[src.to_string_lossy().into_owned()],
            &dest,
            Format::Zip,
            5,
            Some("s3cret"),
        );
        assert!(progress.error.is_none(), "{:?}", progress.error);

        let out = temp_dir("zip-pw-out");
        let locked = round_trip(&dest, &out, None);
        assert_eq!(locked.error.expect("password must be needed").code, "PASSWORD_REQUIRED");
        let wrong = round_trip(&dest, &out.join("wrong"), Some("nope"));
        assert_eq!(wrong.error.expect("wrong password").code, "WRONG_PASSWORD");
        let right = round_trip(&dest, &out.join("right"), Some("s3cret"));
        assert!(right.error.is_none(), "{:?}", right.error);
        assert_eq!(
            fs::read_to_string(out.join("right").join(src.file_name().unwrap()).join("a.txt")).unwrap(),
            "alpha"
        );
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&out);
    }

    #[test]
    fn sevenz_rejects_a_password_instead_of_writing_plain() {
        let src = source("sevenz-pw");
        let dest = src.with_file_name("plain.7z");
        let error = start(
            &OpState::default(),
            Request {
                job_id: "rejected".into(),
                paths: vec![src.to_string_lossy().into_owned()],
                dest: dest.to_string_lossy().into_owned(),
                format: "7z".into(),
                level: 5,
                password: Some("s3cret".into()),
            },
            channel(),
        )
        .expect_err("7z cannot be encrypted by the writer");
        assert_eq!(error.code, "PASSWORD_UNSUPPORTED");
        assert!(!dest.exists());
        let _ = fs::remove_dir_all(&src);
    }

    #[test]
    fn cancel_leaves_no_archive_behind() {
        let src = source("cancel");
        let dest = src.with_file_name("canceled.zip");
        let request = Request {
            job_id: "cancel".into(),
            paths: vec![src.to_string_lossy().into_owned()],
            dest: dest.to_string_lossy().into_owned(),
            format: "zip".into(),
            level: 5,
            password: None,
        };
        let cancel = Arc::new(AtomicBool::new(true));
        let progress = run(request, Format::Zip, 5, cancel, channel());
        assert!(progress.aborted, "{progress:?}", progress = progress.error);
        assert!(progress.error.is_none());
        assert!(!dest.exists(), "a canceled job must not publish a partial archive");
        assert!(!temporary(&dest).exists(), "the temporary must be cleaned up");
        let _ = fs::remove_dir_all(&src);
    }

    #[test]
    fn archive_inside_the_selection_is_not_archived() {
        let src = source("self");
        let dest = src.join("self.zip");
        let progress = compress(&[src.to_string_lossy().into_owned()], &dest, Format::Zip, 5, None);
        assert!(progress.error.is_none(), "{:?}", progress.error);
        let out = temp_dir("self-out");
        let read = round_trip(&dest, &out, None);
        assert!(read.error.is_none(), "{:?}", read.error);
        let root = out.join(src.file_name().unwrap());
        assert!(root.join("a.txt").is_file());
        assert!(!root.join("self.zip").exists(), "the archive cannot contain itself");
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&out);
    }

    #[test]
    fn multiple_selections_land_at_the_archive_root() {
        let dir = temp_dir("multi");
        fs::write(dir.join("one.txt"), b"1").unwrap();
        fs::write(dir.join("two.txt"), b"22").unwrap();
        let dest = dir.join("pair.tar.gz");
        let paths = vec![
            dir.join("two.txt").to_string_lossy().into_owned(),
            dir.join("one.txt").to_string_lossy().into_owned(),
        ];
        let progress = compress(&paths, &dest, Format::TarGz, 1, None);
        assert!(progress.error.is_none(), "{:?}", progress.error);
        let out = temp_dir("multi-out");
        let read = round_trip(&dest, &out, None);
        assert!(read.error.is_none(), "{:?}", read.error);
        assert_eq!(fs::read(out.join("one.txt")).unwrap(), b"1");
        assert_eq!(fs::read(out.join("two.txt")).unwrap(), b"22");
        assert!(!out.join("pair.tar.gz").exists());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&out);
    }

    #[test]
    fn start_reports_progress_and_publishes_the_archive() {
        let src = source("thread");
        let dest = src.with_file_name("thread.zip");
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = seen.clone();
        let channel = Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(json) = body {
                if let Ok(value) = serde_json::from_str(&json) {
                    sink.lock().unwrap().push(value);
                }
            }
            Ok(())
        });
        start(
            &OpState::default(),
            Request {
                job_id: "thread".into(),
                paths: vec![src.to_string_lossy().into_owned()],
                dest: dest.to_string_lossy().into_owned(),
                format: "zip".into(),
                level: 5,
                password: None,
            },
            channel,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !dest.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(dest.is_file(), "the job must publish the archive on its own thread");
        // The final message always arrives after the rename.
        let mut done = false;
        while Instant::now() < deadline && !done {
            done = seen
                .lock()
                .unwrap()
                .iter()
                .any(|progress: &serde_json::Value| progress["done"] == true);
            if !done {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        let messages = seen.lock().unwrap();
        assert!(done, "the job must send a final progress message");
        let last = messages.last().expect("final message");
        assert!(last["error"].is_null(), "{last}");
        assert_eq!(last["phase"], "compress");
        assert!(
            last["total"].as_u64().unwrap_or(0) >= 2,
            "entries were counted: {last}"
        );
        let _ = fs::remove_dir_all(&src);
    }

    #[test]
    fn empty_selection_is_rejected() {
        let error = start(
            &OpState::default(),
            Request {
                job_id: "empty".into(),
                paths: Vec::new(),
                dest: std::env::temp_dir().join("none.zip").to_string_lossy().into_owned(),
                format: "zip".into(),
                level: 5,
                password: None,
            },
            channel(),
        )
        .unwrap_err();
        assert_eq!(error.code, "EMPTY");
    }
}
