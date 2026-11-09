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
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::ipc::Channel;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

use crate::error::ApiError;
use crate::ops::{Kind, OpFailure, OpProgress, Outcome, Request};
use crate::settings::Settings;
use crate::webdav::{self, DavClient};

/// `FILE_ATTRIBUTE_REPARSE_POINT`: junctions and symlinks are skipped during recursive
/// local walks. Following them can leave the selected tree (OneDrive placeholders) or
/// loop forever (a junction pointing at its own ancestor).
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;

fn is_reparse(meta: &std::fs::Metadata) -> bool {
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// True for a directory reached through a junction/symlink. Recursive local walks skip
/// these so a link that points at an ancestor cannot make them loop, and so they do not
/// pull files from outside the selected tree. A file link is not skipped: `fs::copy`
/// follows it and copies the target's contents, matching how the file is opened.
fn skips_reparse_dir(path: &Path, meta: &std::fs::Metadata) -> bool {
    if meta.is_dir() {
        return is_reparse(meta);
    }
    if meta.file_type().is_symlink() {
        return std::fs::metadata(path)
            .map(|target| target.is_dir())
            .unwrap_or(true);
    }
    false
}

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
            landed: Vec::new(),
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
            // WebDAV destinations are deterministic (`targetDir` + source name); the UI's
            // fallback records the same paths.
            landed: Vec::new(),
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
    let remote = webdav::is_webdav(dir);
    let name = if remote {
        safe_remote_name(name)?
    } else {
        safe_local_name(name)?
    };
    runner.label(name);
    if remote {
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
        // A bad connection id or a credential lookup failure fails this item only: the
        // alternatives are aborting the whole batch after some deletes already landed,
        // or reporting success for items that were never attempted.
        let result = (|| -> Result<(), ApiError> {
            if webdav::is_webdav(source) {
                let dav = webdav::parse(source)
                    .ok_or_else(|| ApiError::new("WEBDAV_CONN", source.clone()))?;
                if dav.path.trim_matches('/').is_empty() {
                    // The connection root is the WebDAV equivalent of a volume root; it is
                    // never a delete target even though `protect` skips virtual addresses.
                    return Err(ApiError::new("PROTECTED", source.clone()));
                }
                let client = runner.client(&dav.connection)?;
                client.delete(&dav.path)
            } else {
                // A mixed batch reaches this runner because one source is remote, and its
                // `permanent` flag is chosen for WebDAV (which has no Recycle Bin). Using
                // `std::fs` here would permanently delete the local half without the user
                // asking for it. Refuse until the local half is routed through the shell.
                Err(ApiError::new(
                    "BAD_ARGS",
                    "mixed WebDAV/local delete is not supported",
                ))
            }
        })();
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
            let name = if webdav::is_webdav(source) {
                safe_remote_name(name)?
            } else {
                safe_local_name(name)?
            };
            if webdav::is_webdav(source) {
                let dav = webdav::parse(source)
                    .ok_or_else(|| ApiError::new("WEBDAV_CONN", source.clone()))?;
                // Renaming the connection root would MOVE the whole collection into one
                // of its own children. Delete/Move guard this via `guard_remote_root`;
                // rename has no other caller-side check.
                if dav.path.trim_matches('/').is_empty() {
                    return Err(ApiError::new("PROTECTED", source.clone()));
                }
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
        runner.label(base_name(source));
        let result = (|| -> Result<(), ApiError> {
            // The top-level name is joined to the destination too; a remote displayname
            // such as `C:evil` would otherwise replace the local base path. Derive it
            // here rather than before the loop: a name the target rejects must fail only
            // its own item instead of aborting every remaining source.
            let name = if target_remote {
                safe_remote_name(base_name(source))?
            } else {
                safe_local_name(base_name(source))?
            };
            runner.label(name);
            if target_remote {
                let dav = webdav::parse(target_dir)
                    .ok_or_else(|| ApiError::new("WEBDAV_CONN", target_dir.to_string()))?;
                let client = runner.client(&dav.connection)?;
                let dest_rel = rel_join(&dav.path, name);
                if webdav::is_webdav(source) {
                    let src = webdav::parse(source)
                        .ok_or_else(|| ApiError::new("WEBDAV_CONN", source.to_string()))?;
                    guard_remote_root(&src.path, kind, source)?;
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
                guard_remote_root(&src.path, kind, source)?;
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
    if let Some(existing) = client.stat(dest)? {
        if stat.is_dir {
            if existing.is_dir {
                // COPY/MOVE with `Overwrite: F` cannot express a merge (the server answers
                // 412), and `Overwrite: T` on a collection tells it to DELETE the whole
                // destination first. Walk the source and merge child by child instead.
                // Check the whole tree first: a conflict halfway through a Move leaves
                // the source partially emptied and the item unrecoverable by undo.
                ensure_merge_clear(client, src, dest, overwrite, cancel)?;
                return merge_same_connection(client, src, dest, kind, overwrite, runner, cancel);
            }
            if !overwrite {
                return Err(ApiError::new("EXISTS", client.url_for(dest)));
            }
            // A folder replacing a file is safe to express as a replace.
            client.delete(dest)?;
        } else if existing.is_dir || !overwrite {
            // Replacing a whole collection with one file is never what "overwrite" means
            // in a file manager; report the conflict instead of nuking the folder.
            return Err(ApiError::new("EXISTS", client.url_for(dest)));
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

/// Walks the source tree and rejects a merge that would hit a name conflict, mirroring the
/// per-child rules in [`transfer_same_connection`]. This runs before any child moves, so a
/// failed merge cannot leave the source partially emptied.
fn ensure_merge_clear(
    client: &DavClient,
    src: &str,
    dest: &str,
    overwrite: bool,
    cancel: &AtomicBool,
) -> Result<(), ApiError> {
    check_cancel(cancel)?;
    let (entries, truncated) = client.list_detailed(src, cancel)?;
    if truncated {
        return Err(ApiError::new("TOO_MANY_ENTRIES", client.url_for(src)));
    }
    for entry in entries {
        check_cancel(cancel)?;
        let name = safe_remote_name(&entry.name)?;
        let child_src = rel_join(src, name);
        let child_dest = rel_join(dest, name);
        let Some(existing) = client.stat(&child_dest)? else {
            continue;
        };
        if entry.is_dir() && existing.is_dir {
            ensure_merge_clear(client, &child_src, &child_dest, overwrite, cancel)?;
        } else if !(overwrite && !existing.is_dir) {
            // With overwrite, a file may replace a file and a folder may replace a file;
            // replacing a folder with one file stays a conflict, mirroring
            // `transfer_same_connection`.
            return Err(ApiError::new("EXISTS", client.url_for(&child_dest)));
        }
    }
    Ok(())
}

/// Merges `src` into an existing `dest` collection one child at a time. A move removes the
/// (now empty) source folder after every child landed.
fn merge_same_connection(
    client: &DavClient,
    src: &str,
    dest: &str,
    kind: Kind,
    overwrite: bool,
    runner: &mut Runner,
    cancel: &AtomicBool,
) -> Result<(), ApiError> {
    check_cancel(cancel)?;
    let (entries, truncated) = client.list_detailed(src, cancel)?;
    if truncated {
        // The listing cap hides children; moving them would leave files behind and
        // deleting the source afterwards would lose them.
        return Err(ApiError::new("TOO_MANY_ENTRIES", client.url_for(src)));
    }
    for entry in entries {
        check_cancel(cancel)?;
        let name = safe_remote_name(&entry.name)?;
        transfer_same_connection(
            client,
            &rel_join(src, name),
            &rel_join(dest, name),
            kind,
            overwrite,
            runner,
            cancel,
        )?;
    }
    if kind == Kind::Move {
        // Every child moved out, so this is an empty collection; deleting it avoids a
        // second server-side recursive MOVE that would repeat the overwrite semantics.
        client.delete(src)?;
    }
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
            let bytes = copy_local_tree(src, dest, overwrite, true)?;
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
    let bytes = upload_tree(
        client,
        src,
        dest_rel,
        overwrite,
        kind == Kind::Copy,
        cancel,
        runner,
    )?;
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
        let (entries, truncated) = src.list_detailed(src_rel, cancel)?;
        if truncated {
            // A Move deletes the source after this returns; a cut listing would lose the
            // children it never showed.
            return Err(ApiError::new("TOO_MANY_ENTRIES", src.url_for(src_rel)));
        }
        for entry in entries {
            let name = safe_remote_name(&entry.name)?;
            let child_src = rel_join(src_rel, name);
            let child_dst = rel_join(dst_rel, name);
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
        .get_to_file(src_rel, &temp, cancel, None)
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
    skip_links: bool,
    cancel: &AtomicBool,
    runner: &mut Runner,
) -> Result<u64, ApiError> {
    check_cancel(cancel)?;
    let meta = std::fs::symlink_metadata(local)
        .map_err(|e| ApiError::from_io(&local.to_string_lossy(), &e))?;
    if skips_reparse_dir(local, &meta) {
        // Following it can embed unrelated files or recurse forever. A Move must fail so
        // the caller keeps the source link; a Copy may omit it like the archive writer.
        if skip_links {
            return Ok(0);
        }
        return Err(ApiError::new("SYMLINK_DIR", local.to_string_lossy().into_owned()));
    }
    if meta.is_dir() {
        ensure_remote_dir(client, rel)?;
        let mut bytes = 0u64;
        // An unreadable child must abort the upload: silently skipping it while a Move
        // deletes the source afterwards loses the file.
        let mut entries = std::fs::read_dir(local)
            .map_err(|e| ApiError::from_io(&local.to_string_lossy(), &e))?
            .collect::<Result<Vec<_>, std::io::Error>>()
            .map_err(|e| ApiError::from_io(&local.to_string_lossy(), &e))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            check_cancel(cancel)?;
            // A lossy name would upload under U+FFFD and then let the Move delete the
            // original; refuse the transfer instead.
            let name = entry
                .file_name()
                .to_str()
                .map(str::to_string)
                .ok_or_else(|| ApiError::new("INVALID_NAME", entry.file_name().to_string_lossy()))?;
            let child = entry.path();
            bytes += upload_tree(
                client,
                &child,
                &rel_join(rel, &name),
                overwrite,
                skip_links,
                cancel,
                runner,
            )?;
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
        // A file symlink reports its own (usually zero) length; the uploaded body is the
        // target's, so report that.
        Ok(std::fs::metadata(local).map(|m| m.len()).unwrap_or(meta.len()))
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
        let (entries, truncated) = client.list_detailed(rel, cancel)?;
        if truncated {
            // Same rule as the cross-connection copy: a Move deletes the remote source.
            return Err(ApiError::new("TOO_MANY_ENTRIES", client.url_for(rel)));
        }
        for entry in entries {
            check_cancel(cancel)?;
            let name = safe_local_name(&entry.name)?;
            let child = local.join(name);
            bytes += download_tree(
                client,
                &rel_join(rel, name),
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
        // Stream into a sibling first: a canceled or dropped transfer must never truncate
        // an existing local file. The rename only happens after the body is complete.
        let temp = sibling_temp(local);
        let result = client.get_to_file(rel, &temp, cancel, None);
        match result {
            Ok(written) => {
                if let Err(err) = replace_file(&temp, local) {
                    let _ = std::fs::remove_file(&temp);
                    return Err(ApiError::from_io(&local.to_string_lossy(), &err));
                }
                Ok(written)
            }
            Err(err) => {
                let _ = std::fs::remove_file(&temp);
                Err(err)
            }
        }
    }
}

fn sibling_temp(target: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let name = target
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_string());
    target.with_file_name(format!(
        ".{name}.{}.{}.part",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ))
}

/// `std::fs::rename` refuses to replace an existing file on Windows.
fn replace_file(temp: &Path, target: &Path) -> std::io::Result<()> {
    let from: Vec<u16> = temp.as_os_str().encode_wide().chain([0]).collect();
    let to: Vec<u16> = target.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|_| std::io::Error::last_os_error())
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

fn copy_local_tree(
    src: &Path,
    dest: &Path,
    overwrite: bool,
    skip_links: bool,
) -> Result<u64, ApiError> {
    let meta =
        std::fs::symlink_metadata(src).map_err(|e| ApiError::from_io(&src.to_string_lossy(), &e))?;
    if skips_reparse_dir(src, &meta) {
        // See `upload_tree`: a Move must not delete a source that was never copied.
        if skip_links {
            return Ok(0);
        }
        return Err(ApiError::new("SYMLINK_DIR", src.to_string_lossy().into_owned()));
    }
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
            bytes += copy_local_tree(
                &entry.path(),
                &dest.join(entry.file_name()),
                overwrite,
                skip_links,
            )?;
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
            // the source intact, which is the only safe order. The conflict policy stays
            // the caller's: this fallback must not silently replace a target the user did
            // not agree to overwrite.
            let copied = copy_local_tree(src, dest, overwrite, false)?;
            delete_local(src)?;
            Ok(copied)
        }
    }
}

/// Moving the connection root would delete the whole collection once its children land.
/// `protect` skips virtual addresses, so the runner checks this itself.
fn guard_remote_root(dav_path: &str, kind: Kind, source: &str) -> Result<(), ApiError> {
    if kind == Kind::Move && dav_path.trim_matches('/').is_empty() {
        return Err(ApiError::new("PROTECTED", source.to_string()));
    }
    Ok(())
}

fn guard_into_self_remote(src: &str, dest: &str) -> Result<(), ApiError> {
    let src = src.trim_matches('/');
    let dest = dest.trim_matches('/');
    // The connection root contains every other object, so any same-connection
    // destination lies inside it. Without the empty check the prefix below becomes "/"
    // and never matches the root-relative `dest`, letting a copy/move root into itself
    // through both guards.
    if src.is_empty() || dest == src || dest.starts_with(&format!("{src}/")) {
        return Err(ApiError::new(
            "INVALID_DEST",
            format!("webdav:/{src} -> webdav:/{dest}"),
        ));
    }
    Ok(())
}

fn is_within_local(child: &Path, parent: &Path) -> bool {
    // Reuse the protection guard's key: it resolves `..`, trailing separators, trailing
    // dots/spaces and 8.3 aliases, so `C:\a\..\b` cannot pose as a sibling of `C:\a`.
    let (Some(child), Some(parent)) = (
        crate::protect::key(&child.to_string_lossy()),
        crate::protect::key(&parent.to_string_lossy()),
    ) else {
        return false;
    };
    child == parent || child.starts_with(&format!("{parent}\\"))
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), ApiError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(ApiError::new("CANCELED", "operation canceled"));
    }
    Ok(())
}

/// A remote name is usable only as one path component. Remote servers supply it via
/// `displayname`/href, and rename/new-folder names come from the UI; a separator, `..` or
/// a NUL would escape the destination when joined. `:` is legal on a WebDAV server, so it
/// is only rejected where the name becomes a local file name.
fn safe_remote_name(name: &str) -> Result<&str, ApiError> {
    if name.trim_end_matches([' ', '.']).is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\'])
        || name.contains('\0')
    {
        return Err(ApiError::new("UNSAFE_PATH", name.to_string()));
    }
    Ok(name)
}

/// A name that becomes a local file name also rejects `:`. Win32 reads `C:evil` as a
/// drive-relative path, and joining it replaces the base instead of appending.
fn safe_local_name(name: &str) -> Result<&str, ApiError> {
    safe_remote_name(name)?;
    // Win32 strips trailing dots/spaces, so `a.` and `a ` would silently collide with an
    // existing `a`; `:` would become an alternate data stream or drive-relative path.
    if name.ends_with([' ', '.']) || name.contains(':') {
        return Err(ApiError::new("UNSAFE_PATH", name.to_string()));
    }
    // `CON`, `NUL`, `COM1`, … name devices even with an extension (`NUL.txt`). Writing to
    // such a path silently targets the device instead of the downloaded file.
    let stem = name.split('.').next().unwrap_or(name).to_lowercase();
    const RESERVED: [&str; 22] = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
        "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    if RESERVED.contains(&stem.as_str()) {
        return Err(ApiError::new("UNSAFE_PATH", name.to_string()));
    }
    Ok(name)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_names_cannot_escape_their_destination() {
        for name in ["..", ".", "a/b", "a\\b", "", "   ", "...", "a\0b"] {
            assert!(safe_remote_name(name).is_err(), "{name:?}");
            assert!(safe_local_name(name).is_err(), "{name:?}");
        }
        // `:` is legal on a WebDAV server but becomes a drive-relative path locally.
        assert!(safe_remote_name("report:2024.txt").is_ok());
        assert!(safe_local_name("report:2024.txt").is_err());
        // Win32 strips trailing dots/spaces, so they would collide with the plain name.
        for name in ["a.", "a ", "a. "] {
            assert!(safe_remote_name(name).is_ok(), "{name:?}");
            assert!(safe_local_name(name).is_err(), "{name:?}");
        }
        // Device names stay devices even with an extension; downloading to them would
        // silently target the device instead of writing the file.
        for name in ["CON", "nul", "NUL.txt", "com1.log", "Lpt9"] {
            assert!(safe_remote_name(name).is_ok(), "{name:?}");
            assert!(safe_local_name(name).is_err(), "{name:?}");
        }
        assert!(safe_local_name("console.txt").is_ok());
    }
}
