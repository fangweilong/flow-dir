//! Copy/move/delete/rename implementation for WebDAV paths.
//!
//! `IFileOperation` cannot address a remote server, so `ops::run` routes any batch that
//! touches `webdav://` here. A batch may mix local and remote sources (the in-app clipboard
//! and pane drags can produce one), so local primitives are implemented directly with
//! `std::fs`; the shell path stays reserved for purely local batches.
//!
//! Progress and cancellation use the same [`OpProgress`]/`AtomicBool` contract as the shell
//! runner, which keeps the operation cards and the elevated-retry flow unchanged. WebDAV
//! deletes are permanent (there is no recycle bin), and no retry can supply elevated rights
//! to a remote server, so this runner never records an elevated retry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::ipc::Channel;

use crate::error::ApiError;
use crate::ops::{Kind, OpFailure, OpProgress, Outcome, Request};
use crate::settings::Settings;
use crate::webdav::{self, DavClient};

const MAX_REPORTED_FAILURES: usize = 200;
const PROGRESS_INTERVAL_MS: u128 = 120;

/// Thread-local view of one operation: reuses one WinHTTP session per connection and keeps
/// the per-item bookkeeping in one place.
struct Runner<'a> {
    settings: &'a Settings,
    /// `Rc`, not `Arc`: a `DavClient` owns WinHTTP handles and must stay on the operation
    /// thread, which is also where the runner lives.
    clients: HashMap<String, Rc<DavClient>>,
    progress: Channel<OpProgress>,
    op_id: String,
    phase: String,
    total: u32,
    current: u32,
    name: Option<String>,
    work: u64,
    failed: Vec<OpFailure>,
    failed_total: u32,
    succeeded: Vec<u32>,
    failed_indices: Vec<u32>,
    last_emit: Instant,
}

impl<'a> Runner<'a> {
    fn new(
        settings: &'a Settings,
        request: &Request,
        progress: Channel<OpProgress>,
    ) -> Self {
        Self {
            settings,
            clients: HashMap::new(),
            progress,
            op_id: request.op_id.clone(),
            phase: request.kind.clone(),
            total: request.sources.len().max(1) as u32,
            current: 0,
            name: None,
            work: 0,
            failed: Vec::new(),
            failed_total: 0,
            succeeded: Vec::new(),
            failed_indices: Vec::new(),
            last_emit: Instant::now() - Duration::from_millis(PROGRESS_INTERVAL_MS as u64),
        }
    }

    fn client(&mut self, id: &str) -> Result<Rc<DavClient>, ApiError> {
        if let Some(client) = self.clients.get(id) {
            return Ok(client.clone());
        }
        let connection = self
            .settings
            .webdav
            .iter()
            .find(|conn| conn.id.eq_ignore_ascii_case(id))
            .ok_or_else(|| ApiError::new("WEBDAV_CONN", id.to_string()))?;
        let client = Rc::new(DavClient::new(connection, None)?);
        // One depth-0 probe teaches the client which auth scheme the server wants, so a
        // large upload does not discover a 401 only after its body has been sent. It is
        // best effort: a root that refuses PROPFIND must not block the items, which
        // report their own errors.
        let _ = client.stat("");
        self.clients.insert(id.to_string(), client.clone());
        Ok(client)
    }

    fn label(&mut self, name: &str) {
        self.name = Some(name.to_string());
        self.maybe_emit(false);
    }

    fn add_work(&mut self, bytes: u64) {
        self.work = self.work.saturating_add(bytes);
        self.maybe_emit(false);
    }

    fn item_ok(&mut self, index: u32) {
        self.succeeded.push(index);
        self.current = (self.current + 1).min(self.total);
        self.name = None;
        self.maybe_emit(true);
    }

    fn item_fail(&mut self, index: u32, path: &str, err: &ApiError) {
        self.failed_total += 1;
        self.failed_indices.push(index);
        if self.failed.len() < MAX_REPORTED_FAILURES {
            self.failed.push(OpFailure {
                path: path.to_string(),
                code: err.code.clone(),
                detail: err.detail.clone(),
            });
        }
        self.current = (self.current + 1).min(self.total);
        self.name = None;
        log::warn!(
            target: "flowdir::webdav",
            "item failed code={} path={}",
            err.code,
            path
        );
        self.maybe_emit(true);
    }

    fn maybe_emit(&mut self, force: bool) {
        if !force && self.last_emit.elapsed().as_millis() < PROGRESS_INTERVAL_MS {
            return;
        }
        self.last_emit = Instant::now();
        let _ = self.progress.send(OpProgress {
            op_id: self.op_id.clone(),
            phase: self.phase.clone(),
            current: self.current,
            total: self.total,
            name: self.name.clone(),
            work: self.work,
            work_total: 0,
            failed_count: self.failed_total,
            failed: Vec::new(),
            succeeded: Vec::new(),
            failed_indices: Vec::new(),
            done: false,
            aborted: false,
            error: None,
        });
    }

    fn finish(self, aborted: bool, error: Option<ApiError>) -> Outcome {
        Outcome {
            aborted,
            error,
            failed: self.failed,
            failed_total: self.failed_total,
            succeeded: self.succeeded,
            failed_indices: self.failed_indices,
            denied: false,
        }
    }
}

/// Entry point from `ops::run`. A top-level failure (missing connection, bad arguments)
/// becomes `Outcome.error`; per-item failures accumulate in `Outcome.failed`.
pub(crate) fn run(
    request: &Request,
    kind: Kind,
    cancel: Arc<AtomicBool>,
    progress: Channel<OpProgress>,
    settings: &Settings,
) -> Outcome {
    let mut runner = Runner::new(settings, request, progress);
    let started = Instant::now();
    let result = match kind {
        Kind::NewDir => new_dir(request, &mut runner),
        Kind::Delete => delete_sources(request, &mut runner, &cancel),
        Kind::Rename => rename_sources(request, &mut runner, &cancel),
        Kind::Copy | Kind::Move => transfer_sources(request, kind, &mut runner, &cancel),
    };
    match result {
        Ok(()) => {
            log::info!(
                target: "flowdir::webdav",
                "op {} done: {} items in {}ms",
                request.kind,
                request.sources.len(),
                started.elapsed().as_millis()
            );
            runner.finish(false, None)
        }
        Err(err) if err.code == "CANCELED" => {
            log::warn!(target: "flowdir::webdav", "op {} canceled", request.kind);
            runner.finish(true, None)
        }
        Err(err) => {
            log::error!(
                target: "flowdir::webdav",
                "op {} failed: {} {}",
                request.kind,
                err.code,
                err.detail
            );
            runner.finish(false, Some(err))
        }
    }
}

fn new_dir(request: &Request, runner: &mut Runner) -> Result<(), ApiError> {
    let dir = request
        .target_dir
        .as_deref()
        .ok_or_else(|| ApiError::new("BAD_ARGS", "newdir needs targetDir"))?;
    let name = request
        .new_name
        .as_deref()
        .ok_or_else(|| ApiError::new("BAD_ARGS", "newdir needs newName"))?;
    runner.label(name);
    if webdav::is_webdav(dir) {
        let dav = webdav::parse(dir).ok_or_else(|| ApiError::new("WEBDAV_CONN", dir.to_string()))?;
        let client = runner.client(&dav.connection)?;
        client.mkdir(&rel_join(&dav.path, name))?;
    } else {
        // Only reachable when a mixed batch routed here; keep the local case working
        // instead of failing with a remote error.
        let target = Path::new(dir).join(name);
        std::fs::create_dir(&target).map_err(|e| ApiError::from_io(dir, &e))?;
    }
    runner.item_ok(0);
    Ok(())
}

fn delete_sources(
    request: &Request,
    runner: &mut Runner,
    cancel: &AtomicBool,
) -> Result<(), ApiError> {
    for (index, source) in request.sources.iter().enumerate() {
        check_cancel(cancel)?;
        let index = index as u32;
        runner.label(base_name(source));
        let result = if webdav::is_webdav(source) {
            let dav = webdav::parse(source)
                .ok_or_else(|| ApiError::new("WEBDAV_CONN", source.clone()))?;
            let client = runner.client(&dav.connection)?;
            client.delete(&dav.path)
        } else {
            delete_local(Path::new(source))
        };
        match result {
            Ok(()) => runner.item_ok(index),
            Err(err) if err.code == "CANCELED" => return Err(err),
            Err(err) => runner.item_fail(index, source, &err),
        }
    }
    Ok(())
}

fn rename_sources(
    request: &Request,
    runner: &mut Runner,
    cancel: &AtomicBool,
) -> Result<(), ApiError> {
    let names = rename_names(request)?;
    for (index, source) in request.sources.iter().enumerate() {
        check_cancel(cancel)?;
        let index = index as u32;
        let name = names.get(index as usize).copied().unwrap_or("");
        runner.label(name);
        let result = (|| -> Result<(), ApiError> {
            if webdav::is_webdav(source) {
                let dav = webdav::parse(source)
                    .ok_or_else(|| ApiError::new("WEBDAV_CONN", source.clone()))?;
                let client = runner.client(&dav.connection)?;
                let target = rel_join(rel_parent(&dav.path), name);
                // MOVE with Overwrite: F rejects an existing target with 412 instead of
                // silently replacing it, matching the local rename prompt.
                match client.move_to(&dav.path, &target, request.overwrite) {
                    Ok(()) => {}
                    // A rename on a server without MOVE degrades to copy-then-delete.
                    Err(err) if err.code == "BAD_VERB" => {
                        if !request.overwrite && client.stat(&target)?.is_some() {
                            return Err(ApiError::new("EXISTS", client.url_for(&target)));
                        }
                        copy_between_recursive(
                            &client,
                            &dav.path,
                            &client,
                            &target,
                            request.overwrite,
                            runner,
                            cancel,
                        )?;
                        client.delete(&dav.path)?;
                    }
                    Err(err) => return Err(err),
                }
            } else {
                let parent = Path::new(source)
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default();
                let target = parent.join(name);
                if target.exists() && !request.overwrite {
                    return Err(ApiError::new("EXISTS", target.to_string_lossy().into_owned()));
                }
                std::fs::rename(source, &target)
                    .map_err(|e| ApiError::from_io(source, &e))?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => runner.item_ok(index),
            Err(err) if err.code == "CANCELED" => return Err(err),
            Err(err) => runner.item_fail(index, source, &err),
        }
    }
    Ok(())
}

fn rename_names(request: &Request) -> Result<Vec<&str>, ApiError> {
    match &request.new_names {
        Some(names) => Ok(names.iter().map(String::as_str).collect()),
        None => {
            let name = request
                .new_name
                .as_deref()
                .ok_or_else(|| ApiError::new("BAD_ARGS", "rename needs newName"))?;
            Ok(vec![name; request.sources.len()])
        }
    }
}

fn transfer_sources(
    request: &Request,
    kind: Kind,
    runner: &mut Runner,
    cancel: &AtomicBool,
) -> Result<(), ApiError> {
    let target_dir = request
        .target_dir
        .as_deref()
        .ok_or_else(|| ApiError::new("BAD_ARGS", "copy/move needs targetDir"))?;
    let target_remote = webdav::is_webdav(target_dir);

    for (index, source) in request.sources.iter().enumerate() {
        check_cancel(cancel)?;
        let index = index as u32;
        let name = base_name(source);
        runner.label(name);
        let result = (|| -> Result<(), ApiError> {
            if target_remote {
                let dav = webdav::parse(target_dir)
                    .ok_or_else(|| ApiError::new("WEBDAV_CONN", target_dir.to_string()))?;
                let client = runner.client(&dav.connection)?;
                let dest_rel = rel_join(&dav.path, name);
                if webdav::is_webdav(source) {
                    let src = webdav::parse(source)
                        .ok_or_else(|| ApiError::new("WEBDAV_CONN", source.to_string()))?;
                    let src_client = runner.client(&src.connection)?;
                    if src.connection.eq_ignore_ascii_case(&dav.connection) {
                        guard_into_self_remote(&src.path, &dest_rel)?;
                        transfer_same_connection(
                            &src_client,
                            &src.path,
                            &dest_rel,
                            kind,
                            request.overwrite,
                            runner,
                            cancel,
                        )?;
                    } else {
                        copy_between_connections(
                            &src_client,
                            &src.path,
                            &client,
                            &dest_rel,
                            request.overwrite,
                            runner,
                            cancel,
                        )?;
                        if kind == Kind::Move {
                            src_client.delete(&src.path)?;
                        }
                    }
                } else {
                    transfer_local_to_remote(
                        Path::new(source),
                        &client,
                        &dest_rel,
                        kind,
                        request.overwrite,
                        runner,
                        cancel,
                    )?;
                }
                return Ok(());
            }

            // Local destination.
            let dest = Path::new(target_dir).join(name);
            if webdav::is_webdav(source) {
                let src = webdav::parse(source)
                    .ok_or_else(|| ApiError::new("WEBDAV_CONN", source.to_string()))?;
                let src_client = runner.client(&src.connection)?;
                transfer_remote_to_local(
                    &src_client,
                    &src.path,
                    &dest,
                    kind,
                    request.overwrite,
                    runner,
                    cancel,
                )?;
            } else {
                transfer_local_to_local(
                    Path::new(source),
                    &dest,
                    kind,
                    request.overwrite,
                    runner,
                    cancel,
                )?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => runner.item_ok(index),
            Err(err) if err.code == "CANCELED" => return Err(err),
            Err(err) => runner.item_fail(index, source, &err),
        }
    }
    Ok(())
}

fn transfer_same_connection(
    client: &DavClient,
    src: &str,
    dest: &str,
    kind: Kind,
    overwrite: bool,
    runner: &mut Runner,
    cancel: &AtomicBool,
) -> Result<(), ApiError> {
    check_cancel(cancel)?;
    let stat = client
        .stat(src)?
        .ok_or_else(|| ApiError::new("NOT_FOUND", client.url_for(src)))?;
    if !overwrite {
        if let Some(existing) = client.stat(dest)? {
            // A folder merged into an existing folder keeps Explorer's semantics; a file
            // collision is a failure the UI can report instead of an unannounced replace.
            if !(stat.is_dir && existing.is_dir) {
                return Err(ApiError::new("EXISTS", client.url_for(dest)));
            }
        }
    }
    runner.label(base_name(src));
    let result = match kind {
        Kind::Copy => client.copy_to(src, dest, overwrite),
        Kind::Move => client.move_to(src, dest, overwrite),
        _ => return Err(ApiError::new("BAD_ARGS", "unsupported transfer")),
    };
    match result {
        Ok(()) => {}
        // RFC 4918 requires COPY/MOVE, but lightweight NAS stacks and proxy front-ends
        // sometimes answer 405/501 for them. Falling back to a byte round trip keeps the
        // transfer working; the recursion reports per-file progress and cancellation.
        Err(err) if err.code == "BAD_VERB" => {
            copy_between_recursive(client, src, client, dest, overwrite, runner, cancel)?;
            if kind == Kind::Move {
                // Same order as every other move path: delete the source only after the
                // destination holds every byte.
                client.delete(src)?;
            }
            return Ok(());
        }
        Err(err) => return Err(err),
    }
    runner.add_work(stat.size);
    Ok(())
}

fn transfer_local_to_local(
    src: &Path,
    dest: &Path,
    kind: Kind,
    overwrite: bool,
    runner: &mut Runner,
    cancel: &AtomicBool,
) -> Result<(), ApiError> {
    check_cancel(cancel)?;
    if is_within_local(dest, src) {
        return Err(ApiError::new("INVALID_DEST", dest.to_string_lossy().into_owned()));
    }
    match kind {
        Kind::Copy => {
            let bytes = copy_local_tree(src, dest, overwrite)?;
            runner.add_work(bytes);
        }
        Kind::Move => {
            let bytes = move_local_tree(src, dest, overwrite)?;
            runner.add_work(bytes);
        }
        _ => return Err(ApiError::new("BAD_ARGS", "unsupported transfer")),
    }
    Ok(())
}

fn transfer_local_to_remote(
    src: &Path,
    client: &DavClient,
    dest_rel: &str,
    kind: Kind,
    overwrite: bool,
    runner: &mut Runner,
    cancel: &AtomicBool,
) -> Result<(), ApiError> {
    check_cancel(cancel)?;
    let meta = std::fs::metadata(src).map_err(|e| ApiError::from_io(&src.to_string_lossy(), &e))?;
    if !overwrite {
        if let Some(existing) = client.stat(dest_rel)? {
            if !(meta.is_dir() && existing.is_dir) {
                return Err(ApiError::new("EXISTS", client.url_for(dest_rel)));
            }
        }
    }
    let bytes = upload_tree(client, src, dest_rel, overwrite, cancel, runner)?;
    runner.add_work(bytes);
    if kind == Kind::Move {
        // Delete only after every byte landed; the reverse order could lose the source if
        // the upload failed halfway.
        delete_local(src)?;
    }
    Ok(())
}

fn transfer_remote_to_local(
    client: &DavClient,
    src_rel: &str,
    dest: &Path,
    kind: Kind,
    overwrite: bool,
    runner: &mut Runner,
    cancel: &AtomicBool,
) -> Result<(), ApiError> {
    check_cancel(cancel)?;
    let stat = client
        .stat(src_rel)?
        .ok_or_else(|| ApiError::new("NOT_FOUND", client.url_for(src_rel)))?;
    if !overwrite && dest.exists() && !(stat.is_dir && dest.is_dir()) {
        return Err(ApiError::new("EXISTS", dest.to_string_lossy().into_owned()));
    }
    let bytes = download_tree(client, src_rel, dest, overwrite, cancel, runner)?;
    runner.add_work(bytes);
    if kind == Kind::Move {
        client.delete(src_rel)?;
    }
    Ok(())
}

fn copy_between_connections(
    src: &DavClient,
    src_rel: &str,
    dst: &DavClient,
    dst_rel: &str,
    overwrite: bool,
    runner: &mut Runner,
    cancel: &AtomicBool,
) -> Result<(), ApiError> {
    copy_between_recursive(src, src_rel, dst, dst_rel, overwrite, runner, cancel)?;
    Ok(())
}

fn copy_between_recursive(
    src: &DavClient,
    src_rel: &str,
    dst: &DavClient,
    dst_rel: &str,
    overwrite: bool,
    runner: &mut Runner,
    cancel: &AtomicBool,
) -> Result<(), ApiError> {
    check_cancel(cancel)?;
    let stat = src
        .stat(src_rel)?
        .ok_or_else(|| ApiError::new("NOT_FOUND", src.url_for(src_rel)))?;
    if stat.is_dir {
        ensure_remote_dir(dst, dst_rel)?;
        for entry in src.list(src_rel, cancel)? {
            let child_src = rel_join(src_rel, &entry.name);
            let child_dst = rel_join(dst_rel, &entry.name);
            copy_between_recursive(src, &child_src, dst, &child_dst, overwrite, runner, cancel)?;
        }
        return Ok(());
    }

    if !overwrite && dst.stat(dst_rel)?.is_some() {
        return Err(ApiError::new("EXISTS", dst.url_for(dst_rel)));
    }
    // The two servers have no shared filesystem and no server-side COPY, so the file
    // makes a round trip through the temp directory rather than through memory.
    let temp = temp_path();
    let result = src
        .get_to_file(src_rel, &temp, cancel)
        .and_then(|_| dst.upload_file(dst_rel, &temp, cancel));
    let _ = std::fs::remove_file(&temp);
    result?;
    runner.add_work(stat.size);
    Ok(())
}

fn upload_tree(
    client: &DavClient,
    local: &Path,
    rel: &str,
    overwrite: bool,
    cancel: &AtomicBool,
    runner: &mut Runner,
) -> Result<u64, ApiError> {
    check_cancel(cancel)?;
    let meta = std::fs::metadata(local).map_err(|e| ApiError::from_io(&local.to_string_lossy(), &e))?;
    if meta.is_dir() {
        ensure_remote_dir(client, rel)?;
        let mut bytes = 0u64;
        let mut entries: Vec<_> = std::fs::read_dir(local)
            .map_err(|e| ApiError::from_io(&local.to_string_lossy(), &e))?
            .filter_map(Result::ok)
            .collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            check_cancel(cancel)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = entry.path();
            bytes += upload_tree(client, &child, &rel_join(rel, &name), overwrite, cancel, runner)?;
        }
        Ok(bytes)
    } else {
        // A folder merge must not silently replace a same-named remote file: report the
        // conflict like the local copy path does, leaving the rest to the user.
        if !overwrite && client.stat(rel)?.is_some() {
            return Err(ApiError::new("EXISTS", client.url_for(rel)));
        }
        let name = local
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        runner.label(&name);
        client.upload_file(rel, local, cancel)?;
        Ok(meta.len())
    }
}

fn download_tree(
    client: &DavClient,
    rel: &str,
    local: &Path,
    overwrite: bool,
    cancel: &AtomicBool,
    runner: &mut Runner,
) -> Result<u64, ApiError> {
    check_cancel(cancel)?;
    let stat = client
        .stat(rel)?
        .ok_or_else(|| ApiError::new("NOT_FOUND", client.url_for(rel)))?;
    if stat.is_dir {
        std::fs::create_dir_all(local)
            .map_err(|e| ApiError::from_io(&local.to_string_lossy(), &e))?;
        let mut bytes = 0u64;
        for entry in client.list(rel, cancel)? {
            check_cancel(cancel)?;
            let child = local.join(&entry.name);
            bytes += download_tree(
                client,
                &rel_join(rel, &entry.name),
                &child,
                overwrite,
                cancel,
                runner,
            )?;
        }
        Ok(bytes)
    } else {
        // Same conflict rule as upload: a merge does not replace existing files unless
        // the caller asked for it.
        if !overwrite && local.exists() {
            return Err(ApiError::new("EXISTS", local.to_string_lossy().into_owned()));
        }
        let name = local
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        runner.label(&name);
        if let Some(parent) = local.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ApiError::from_io(&parent.to_string_lossy(), &e))?;
        }
        let written = client.get_to_file(rel, local, cancel)?;
        Ok(written)
    }
}

fn ensure_remote_dir(client: &DavClient, rel: &str) -> Result<(), ApiError> {
    match client.stat(rel)? {
        Some(stat) if stat.is_dir => Ok(()),
        Some(_) => Err(ApiError::new("EXISTS", client.url_for(rel))),
        None => client.mkdir(rel),
    }
}

fn delete_local(path: &Path) -> Result<(), ApiError> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| ApiError::from_io(&path.to_string_lossy(), &e))?;
    let result = if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    result.map_err(|e| ApiError::from_io(&path.to_string_lossy(), &e))
}

fn copy_local_tree(src: &Path, dest: &Path, overwrite: bool) -> Result<u64, ApiError> {
    let meta = std::fs::metadata(src).map_err(|e| ApiError::from_io(&src.to_string_lossy(), &e))?;
    if meta.is_dir() {
        if dest.exists() && !dest.is_dir() {
            return Err(ApiError::new("EXISTS", dest.to_string_lossy().into_owned()));
        }
        std::fs::create_dir_all(dest)
            .map_err(|e| ApiError::from_io(&dest.to_string_lossy(), &e))?;
        let mut bytes = 0u64;
        for entry in std::fs::read_dir(src)
            .map_err(|e| ApiError::from_io(&src.to_string_lossy(), &e))?
        {
            let entry = entry.map_err(|e| ApiError::from_io(&src.to_string_lossy(), &e))?;
            bytes += copy_local_tree(&entry.path(), &dest.join(entry.file_name()), overwrite)?;
        }
        Ok(bytes)
    } else {
        if dest.exists() && !overwrite {
            return Err(ApiError::new("EXISTS", dest.to_string_lossy().into_owned()));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ApiError::from_io(&parent.to_string_lossy(), &e))?;
        }
        std::fs::copy(src, dest).map_err(|e| ApiError::from_io(&src.to_string_lossy(), &e))?;
        Ok(meta.len())
    }
}

fn move_local_tree(src: &Path, dest: &Path, overwrite: bool) -> Result<u64, ApiError> {
    if dest.exists() && !overwrite {
        let src_dir = src.is_dir();
        if !(src_dir && dest.is_dir()) {
            return Err(ApiError::new("EXISTS", dest.to_string_lossy().into_owned()));
        }
    }
    let bytes = std::fs::metadata(src).map(|meta| meta.len()).unwrap_or(0);
    match std::fs::rename(src, dest) {
        Ok(()) => Ok(bytes),
        Err(_) => {
            // Different volume: copy first, then remove the source. A failed copy leaves
            // the source intact, which is the only safe order.
            let copied = copy_local_tree(src, dest, true)?;
            delete_local(src)?;
            Ok(copied)
        }
    }
}

fn guard_into_self_remote(src: &str, dest: &str) -> Result<(), ApiError> {
    if dest == src || dest.starts_with(&format!("{}/", src.trim_end_matches('/'))) {
        return Err(ApiError::new(
            "INVALID_DEST",
            format!("webdav:/{src} -> webdav:/{dest}"),
        ));
    }
    Ok(())
}

fn is_within_local(child: &Path, parent: &Path) -> bool {
    let child = child.to_string_lossy().to_lowercase();
    let parent = parent.to_string_lossy().to_lowercase().replace('/', "\\");
    child == parent || child.starts_with(&format!("{parent}\\"))
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), ApiError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(ApiError::new("CANCELED", "operation canceled"));
    }
    Ok(())
}

fn rel_join(base: &str, name: &str) -> String {
    let base = base.trim_matches('/');
    let name = name.trim_matches('/');
    if base.is_empty() {
        name.to_string()
    } else if name.is_empty() {
        base.to_string()
    } else {
        format!("{base}/{name}")
    }
}

fn rel_parent(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("")
}

fn base_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
}

fn temp_path() -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "flowdir-webdav-{}-{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ))
}
