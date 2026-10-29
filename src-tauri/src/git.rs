//! Repository status for the file list (`git_status`).
//!
//! Shells out to the installed `git` binary instead of linking a second object model:
//! one porcelain run per folder is cheap, honours the user's config (fsmonitor,
//! large-repo settings, submodules) and needs no extra dependency.
//!
//! Listing never blocks on it: the command runs off the UI thread and the list paints
//! first, badges arrive afterwards.

use std::collections::HashMap;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;


const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// The listed folder's own status; also the `rev-parse` probe.
const STATUS_TIMEOUT: Duration = Duration::from_secs(8);
/// Child repository summaries: bounded so a stuck folder cannot delay the listed one.
const CHILD_TIMEOUT: Duration = Duration::from_secs(3);
/// Folders holding more repositories than this keep the plain marker on the rest.
const CHILD_REPO_LIMIT: usize = 64;
const CHILD_WORKERS: usize = 4;

#[derive(Serialize, Clone)]
pub struct GitFile {
    /// Work-tree relative path with `\` separators, so the UI only joins.
    pub path: String,
    /// One display letter: M, A, D, R, C, T, U (untracked) or ! (conflict).
    pub code: String,
}

#[derive(Serialize, Clone, Default)]
pub struct GitRepo {
    /// Direct child folder name.
    pub name: String,
    /// False when the per-repository probe timed out or failed; the row then shows no state.
    pub checked: bool,
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
    pub upstream: bool,
    /// Any unmerged record in the index or work tree.
    pub conflict: bool,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    /// Work-tree root; empty when the folder is not inside a repository.
    pub root: String,
    /// Changed entries that are direct children of the folder.
    pub files: Vec<GitFile>,
    /// Child folders holding changes deeper down; they only get a marker, not a letter.
    pub dirs: Vec<String>,
    /// Tracked direct child folders with no change anywhere below them.
    pub clean_dirs: Vec<String>,
    /// Checked-out branch; empty when HEAD is detached.
    pub branch: String,
    /// Commits the branch is ahead of and behind its upstream; both zero without one.
    pub ahead: u32,
    pub behind: u32,
    /// Whether the branch has an upstream, so the counts mean anything.
    pub upstream: bool,
    /// Any change anywhere in the work tree, not only below the listed folder.
    pub dirty: bool,
    /// Direct child folders that are repository roots, with their own summary so the row
    /// badge can show check / pending / unpushed without opening the repository.
    pub repos: Vec<GitRepo>,
}

/// Repositories whose porcelain output is reused while navigating inside them.
const STATUS_CACHE_TTL: Duration = Duration::from_secs(2);
const CHILD_CACHE_TTL: Duration = Duration::from_secs(5);
const CACHE_ENTRIES: usize = 16;

struct RootData {
    at: Instant,
    output: Arc<Vec<u8>>,
    /// Tracked paths from the repository root, `/` separated.
    tracked: Arc<Vec<String>>,
}

type ChildCacheEntry = (Instant, Vec<GitRepo>);

static ROOT_CACHE: LazyLock<Mutex<HashMap<String, RootData>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static CHILD_CACHE: LazyLock<Mutex<HashMap<String, ChildCacheEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Drops cached status for a repository root, called when the watcher reports a change
/// there so the next listing is fresh instead of waiting out the TTL.
pub fn invalidate(path: &str) {
    let key = path.to_lowercase();
    if let Ok(mut cache) = ROOT_CACHE.lock() {
        cache.retain(|root, _| !key.starts_with(root.as_str()));
    }
    if let Ok(mut cache) = CHILD_CACHE.lock() {
        cache.remove(&key);
    }
}

pub fn status(path: &str) -> GitStatus {
    let names = child_repos(path);
    let Some(git) = git_binary() else {
        return GitStatus {
            repos: unchecked_repos(names),
            ..GitStatus::default()
        };
    };
    let repos = repo_states(git, path, names);
    let Some(root) = toplevel(git, path) else {
        return GitStatus {
            repos,
            ..GitStatus::default()
        };
    };
    let prefix = relative_to_root(&root, path);
    let Some(data) = root_data(git, &root) else {
        return GitStatus {
            root,
            repos,
            ..GitStatus::default()
        };
    };
    let (branch, ahead, behind, upstream) = summary(&data.output);
    let dirty = has_changes(&data.output);
    // Nested paths collapse to one marker per child folder: a work tree with 50k changes
    // below the listed folder must not push 50k strings through the IPC bridge.
    let (files, dirs) = split_direct(parse(&data.output, prefix.as_deref()));
    let clean_dirs = clean_dirs(&data.tracked, prefix.as_deref(), &dirs);
    GitStatus {
        root,
        files,
        dirs,
        clean_dirs,
        branch,
        ahead,
        behind,
        upstream,
        dirty,
        repos,
    }
}

/// One porcelain run per repository root, reused by every folder inside it. The watcher
/// only needs to push a change into the list; the short TTL also covers changes made
/// outside the app without a per-pane invalidation round-trip.
fn root_data(git: &Path, root: &str) -> Option<RootData> {
    let key = root.to_lowercase();
    if let Ok(cache) = ROOT_CACHE.lock() {
        if let Some(entry) = cache.get(&key) {
            if entry.at.elapsed() < STATUS_CACHE_TTL {
                return Some(RootData {
                    at: entry.at,
                    output: entry.output.clone(),
                    tracked: entry.tracked.clone(),
                });
            }
        }
    }
    let mut command = status_command(git, Path::new(root), "all");
    let output = output_with_timeout(&mut command, STATUS_TIMEOUT)?;
    if !output.status.success() {
        return None;
    }
    let tracked = Arc::new(tracked_paths(git, root));
    let data = RootData {
        at: Instant::now(),
        output: Arc::new(output.stdout),
        tracked,
    };
    if let Ok(mut cache) = ROOT_CACHE.lock() {
        cache.retain(|_, entry| entry.at.elapsed() < STATUS_CACHE_TTL);
        if cache.len() >= CACHE_ENTRIES {
            cache.clear();
        }
        cache.insert(
            key,
            RootData {
                at: data.at,
                output: data.output.clone(),
                tracked: data.tracked.clone(),
            },
        );
    }
    Some(data)
}

fn tracked_paths(git: &Path, root: &str) -> Vec<String> {
    let mut command = Command::new(git);
    command
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z"])
        .creation_flags(CREATE_NO_WINDOW);
    let Some(output) = output_with_timeout(&mut command, STATUS_TIMEOUT) else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).replace('/', "\\"))
        .collect()
}

/// Direct child folders that hold at least one tracked file, minus the dirty ones.
/// Only tracked directories get the check; a folder full of ignored files stays plain.
fn clean_dirs(tracked: &[String], prefix: Option<&str>, dirty: &[String]) -> Vec<String> {
    let dirty: std::collections::HashSet<String> =
        dirty.iter().map(|dir| dir.to_lowercase()).collect();
    let mut dirs: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for path in tracked {
        let relative = match prefix {
            None => path.as_str(),
            Some(prefix) => match path.get(..prefix.len()) {
                Some(head)
                    if head.eq_ignore_ascii_case(prefix)
                        && path.as_bytes().get(prefix.len()) == Some(&b'\\') =>
                {
                    &path[prefix.len() + 1..]
                }
                _ => continue,
            },
        };
        let Some((dir, rest)) = relative.split_once('\\') else {
            continue;
        };
        if rest.is_empty() || dirty.contains(&dir.to_lowercase()) {
            continue;
        }
        if seen.insert(dir.to_lowercase()) {
            dirs.push(dir.to_string());
        }
    }
    dirs.sort_by_key(|dir| dir.to_lowercase());
    dirs
}

/// `git` is normally on PATH; GUI launches sometimes miss the user's shims, so the two
/// standard install locations are tried once and the answer is cached for the process.
static GIT: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    let mut candidates = vec![PathBuf::from("git")];
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        candidates.push(PathBuf::from(program_files).join(r"Git\cmd\git.exe"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join(r"Programs\Git\cmd\git.exe"));
    }
    candidates.into_iter().find(|candidate| {
        let mut command = Command::new(candidate);
        command.arg("--version").creation_flags(CREATE_NO_WINDOW);
        output_with_timeout(&mut command, STATUS_TIMEOUT).is_some_and(|out| out.status.success())
    })
});

fn git_binary() -> Option<&'static PathBuf> {
    GIT.as_ref()
}

fn toplevel(git: &PathBuf, dir: &str) -> Option<String> {
    let mut command = Command::new(git);
    let output = output_with_timeout(
        command
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", "--show-toplevel"])
            .creation_flags(CREATE_NO_WINDOW),
        STATUS_TIMEOUT,
    )?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let root = text.trim().trim_end_matches(['/', '\\']).replace('/', "\\");
    (!root.is_empty()).then_some(root)
}

/// Direct child folders holding git metadata. A probe per child is enough to mark the row:
/// the folder is a repository root whether `.git` is a directory or the pointer file used
/// by linked worktrees and submodules.
fn child_repos(dir: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut repos: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| {
            let dot_git = entry.path().join(".git");
            dot_git.is_dir() || dot_git.is_file()
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    repos.sort_by_key(|name| name.to_lowercase());
    repos
}

/// One porcelain status with the branch header; the untracked mode trades detail for speed.
fn status_command(git: &Path, dir: &Path, untracked: &str) -> Command {
    let mut command = Command::new(git);
    command
        .arg("--no-optional-locks")
        .args(["-c", "core.quotepath=false"])
        .arg("-C")
        .arg(dir)
        .args(["status", "--porcelain=v1", "-z", "--branch"])
        // The value must be attached: `--untracked-files all` would treat `all` as a
        // pathspec and silently report only the branch header.
        .arg(format!("--untracked-files={untracked}"))
        .env("GIT_OPTIONAL_LOCKS", "0")
        .creation_flags(CREATE_NO_WINDOW);
    command
}

fn unchecked_repos(names: Vec<String>) -> Vec<GitRepo> {
    names
        .into_iter()
        .map(|name| GitRepo {
            name,
            ..GitRepo::default()
        })
        .collect()
}

/// Summarises every direct child repository, four at a time so a folder full of projects
/// still answers quickly. The first `CHILD_REPO_LIMIT` are probed; the rest keep the plain
/// marker instead of a state badge. Order follows the sorted input.
fn repo_states(git: &Path, parent: &str, names: Vec<String>) -> Vec<GitRepo> {
    let mut repos = unchecked_repos(names.clone());
    // Re-entering a folder must not re-probe every child repository: a short TTL keeps
    // navigation instant while still catching edits made outside the app.
    let key = parent.to_lowercase();
    if let Ok(cache) = CHILD_CACHE.lock() {
        if let Some((at, cached)) = cache.get(&key) {
            if at.elapsed() < CHILD_CACHE_TTL && cached.len() == names.len() {
                return cached.clone();
            }
        }
    }
    let checked = names.len().min(CHILD_REPO_LIMIT);
    if checked == 0 {
        return repos;
    }
    let parent = Path::new(parent);
    let workers = CHILD_WORKERS.min(checked);
    let names = &names;
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|worker| {
                scope.spawn(move || {
                    (worker..checked)
                        .step_by(workers)
                        .map(|index| (index, repo_state(git, &parent.join(&names[index]))))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for handle in handles {
            if let Ok(items) = handle.join() {
                for (index, state) in items {
                    repos[index] = state;
                }
            }
        }
    });
    if let Ok(mut cache) = CHILD_CACHE.lock() {
        cache.retain(|_, (at, _)| at.elapsed() < CHILD_CACHE_TTL);
        if cache.len() >= CACHE_ENTRIES {
            cache.clear();
        }
        cache.insert(key, (Instant::now(), repos.clone()));
    }
    repos
}

fn repo_state(git: &Path, dir: &Path) -> GitRepo {
    let mut state = GitRepo {
        name: dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        ..GitRepo::default()
    };
    let mut command = status_command(git, dir, "normal");
    let Some(output) = output_with_timeout(&mut command, CHILD_TIMEOUT) else {
        return state;
    };
    if !output.status.success() {
        return state;
    }
    let (_, ahead, behind, upstream) = summary(&output.stdout);
    state.checked = true;
    state.dirty = has_changes(&output.stdout);
    state.conflict = has_conflict(&output.stdout);
    state.ahead = ahead;
    state.behind = behind;
    state.upstream = upstream;
    state
}

/// Runs `command` with a hard deadline. A work tree on an unreachable share (or a stuck
/// `core.fsmonitor` hook) would otherwise pin a blocking thread forever; `None` means the
/// child was killed. Output is drained on threads because status on a large repository
/// writes far more than a pipe buffer.
fn output_with_timeout(command: &mut Command, timeout: Duration) -> Option<Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let out = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let err = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Some(Output {
                    status,
                    stdout: out.join().ok()?,
                    stderr: err.join().ok()?,
                })
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(_) => return None,
        }
    }
}

/// Splits pruned status paths into direct children and child folders, dropping duplicates
/// and deeper paths. A path with any separator, including the trailing one of a collapsed
/// folder record, marks its first component as a folder: only a real file gets a letter.
fn split_direct(files: Vec<GitFile>) -> (Vec<GitFile>, Vec<String>) {
    let mut direct = Vec::new();
    let mut dirs: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for file in files {
        match file.path.find('\\') {
            None => direct.push(file),
            Some(cut) => {
                let dir = file.path[..cut].to_string();
                if seen.insert(dir.to_lowercase()) {
                    dirs.push(dir);
                }
            }
        }
    }
    dirs.sort_by_key(|dir| dir.to_lowercase());
    (direct, dirs)
}

/// Path of `dir` relative to the work-tree root, or `None` when `dir` *is* the root.
/// Comparing ASCII case-insensitively is enough: both strings come from the same volume.
fn relative_to_root(root: &str, dir: &str) -> Option<String> {
    let root = root.trim_end_matches(['\\', '/']);
    let dir = dir.trim_end_matches(['\\', '/']);
    if dir.len() <= root.len() || !dir.get(..root.len()).is_some_and(|head| head.eq_ignore_ascii_case(root)) {
        return None;
    }
    let rest = dir.get(root.len()..).unwrap_or_default().trim_start_matches(['\\', '/']);
    let rest = rest.replace('/', "\\");
    (!rest.is_empty()).then_some(rest)
}

/// Reads the `--branch` header: `## head...upstream [ahead N, behind M]`.
/// `head` is `HEAD (no branch)` for a detached checkout and
/// `No commits yet on <name>` right after `git init`.
fn summary(bytes: &[u8]) -> (String, u32, u32, bool) {
    let Some(record) = bytes
        .split(|byte| *byte == 0)
        .find(|record| record.starts_with(b"## "))
    else {
        return (String::new(), 0, 0, false);
    };
    let line = String::from_utf8_lossy(&record[3..]).into_owned();
    let (head, tracking) = line.split_once("...").unwrap_or((&line, ""));
    let head = head.trim();
    let branch = match head.strip_prefix("No commits yet on ") {
        Some(name) => name.to_string(),
        None if head == "HEAD (no branch)" => String::new(),
        None => head.to_string(),
    };
    let mut ahead = 0;
    let mut behind = 0;
    if let Some(open) = tracking.find('[') {
        let counts = tracking[open + 1..].split(']').next().unwrap_or_default();
        for part in counts.split(',') {
            let mut words = part.split_whitespace();
            match (words.next(), words.next().and_then(|count| count.parse().ok())) {
                (Some("ahead"), Some(count)) => ahead = count,
                (Some("behind"), Some(count)) => behind = count,
                _ => {}
            }
        }
    }
    (branch, ahead, behind, !tracking.is_empty())
}

/// Any record besides the branch header means the work tree has something to report.
fn has_changes(bytes: &[u8]) -> bool {
    bytes
        .split(|byte| *byte == 0)
        .any(|record| !record.is_empty() && !record.starts_with(b"## "))
}

/// Unmerged porcelain states: `UU`, `AA`, `DD`, or any side marked `U`.
fn has_conflict(bytes: &[u8]) -> bool {
    bytes.split(|byte| *byte == 0).any(|record| {
        if record.len() < 4 || record.starts_with(b"## ") {
            return false;
        }
        let index = record[0] as char;
        let worktree = record[1] as char;
        index == 'U'
            || worktree == 'U'
            || (index == 'A' && worktree == 'A')
            || (index == 'D' && worktree == 'D')
    })
}

/// Parses `--porcelain=v1 -z`: NUL separated `XY path` records, where renames and copies
/// carry the source path as one extra record.
///
/// `prefix` keeps only entries inside the requested folder and makes their paths relative
/// to it, so the reply stays small in repositories with thousands of changes.
fn parse(bytes: &[u8], prefix: Option<&str>) -> Vec<GitFile> {
    let mut files = Vec::new();
    let mut records = bytes.split(|byte| *byte == 0).filter(|r| !r.is_empty());
    while let Some(record) = records.next() {
        // `--branch` prepends one `## head...upstream` record; it is not a file entry.
        if record.starts_with(b"## ") || record.len() < 4 {
            continue;
        }
        let status = String::from_utf8_lossy(&record[..2]).into_owned();
        // A record ending in `/` names a folder (an untracked foreign repository, which
        // even `--untracked-files=all` does not descend into). The separator is kept so
        // `split_direct` can give that row a folder marker instead of a status letter.
        let path = String::from_utf8_lossy(&record[3..]).replace('/', "\\").to_string();
        if status.starts_with('R') || status.starts_with('C') {
            records.next();
        }
        let relative = match prefix {
            None => Some(path),
            Some(prefix) => match path.get(..prefix.len()) {
                Some(head) if head.eq_ignore_ascii_case(prefix) && path.as_bytes().get(prefix.len()) == Some(&b'\\') => {
                    Some(path[prefix.len() + 1..].to_string())
                }
                _ => None,
            },
        };
        if let Some(path) = relative.filter(|path| !path.is_empty()) {
            files.push(GitFile {
                path,
                code: code_for(&status),
            });
        }
    }
    files
}

fn code_for(status: &str) -> String {
    let mut chars = status.chars();
    let index = chars.next().unwrap_or(' ');
    let worktree = chars.next().unwrap_or(' ');
    // Unmerged combinations (UU, AA, DD, AU, …) mean a conflict, not an edit.
    if index == 'U' || worktree == 'U' || (index == 'A' && worktree == 'A') || (index == 'D' && worktree == 'D') {
        return "!".to_string();
    }
    if index == '?' {
        return "U".to_string();
    }
    let code = if worktree != ' ' { worktree } else { index };
    code.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_records_map_to_letters() {
        // The `--branch` header is not a file entry.
        let raw = b"## main...origin/main [ahead 1]\0 M src\\App.tsx\0?? new file.txt\0A  staged.txt\0 D gone.txt\0UU merge.txt\0R  renamed.txt\0old.txt\0 M dir\\nested.txt\0";
        let files = parse(raw, None);
        assert_eq!(files.len(), 7);
        let pairs: Vec<(&str, &str)> = files
            .iter()
            .map(|f| (f.path.as_str(), f.code.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("src\\App.tsx", "M"),
                ("new file.txt", "U"),
                ("staged.txt", "A"),
                ("gone.txt", "D"),
                ("merge.txt", "!"),
                ("renamed.txt", "R"),
                ("dir\\nested.txt", "M"),
            ]
        );
    }

    #[test]
    fn branch_header_reports_tracking_counts() {
        assert_eq!(
            summary(b"## main...origin/main [ahead 2, behind 3]\0"),
            ("main".to_string(), 2, 3, true)
        );
        assert_eq!(
            summary(b"## main...origin/main\0"),
            ("main".to_string(), 0, 0, true)
        );
        assert_eq!(summary(b"## fix/x\0"), ("fix/x".to_string(), 0, 0, false));
        assert_eq!(
            summary(b"## HEAD (no branch)\0"),
            (String::new(), 0, 0, false)
        );
        assert_eq!(
            summary(b"## No commits yet on main\0"),
            ("main".to_string(), 0, 0, false)
        );
        // An upstream that was deleted still counts as configured.
        assert_eq!(
            summary(b"## main...origin/main [gone]\0"),
            ("main".to_string(), 0, 0, true)
        );
        assert_eq!(summary(b""), (String::new(), 0, 0, false));
    }

    #[test]
    fn any_file_record_marks_the_work_tree_dirty() {
        assert!(!has_changes(b""));
        assert!(!has_changes(b"## main...origin/main\0"));
        assert!(has_changes(b"## main...origin/main\0 M a.txt\0"));
        assert!(has_changes(b"?? new.txt\0"));
    }

    #[test]
    fn untracked_folder_records_mark_the_folder_instead_of_a_letter() {
        // `--untracked-files=all` still collapses an untracked foreign repository.
        let (direct, dirs) = split_direct(parse(b"?? build/\0", None));
        assert!(direct.is_empty());
        assert_eq!(dirs, ["build"]);
    }

    #[test]
    fn untracked_files_inside_a_new_folder_keep_their_status() {
        let raw = b"?? newdir\\a.txt\0?? newdir\\deep\\b.txt\0";
        let files = parse(raw, Some("newdir"));
        let pairs: Vec<(&str, &str)> = files
            .iter()
            .map(|f| (f.path.as_str(), f.code.as_str()))
            .collect();
        assert_eq!(pairs, [("a.txt", "U"), ("deep\\b.txt", "U")]);

        let (direct, dirs) = split_direct(files);
        assert_eq!(direct.len(), 1);
        assert_eq!(direct[0].path, "a.txt");
        assert_eq!(dirs, ["deep"]);
    }

    #[test]
    fn prefix_keeps_the_subtree_and_strips_it() {
        let raw = b" M src\\App.tsx\0 M src\\tauri\\lib.rs\0 M README.md\0?? src\\new.txt\0";
        let files = parse(raw, Some("src"));
        let pairs: Vec<(&str, &str)> = files
            .iter()
            .map(|f| (f.path.as_str(), f.code.as_str()))
            .collect();
        assert_eq!(pairs, [("App.tsx", "M"), ("tauri\\lib.rs", "M"), ("new.txt", "U")]);
        // Case differs on purpose: drive and folder casing comes from two sources.
        assert_eq!(parse(raw, Some("SRC")).len(), 3);

        let (direct, dirs) = split_direct(parse(raw, Some("src")));
        let names: Vec<&str> = direct.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names, ["App.tsx", "new.txt"]);
        assert_eq!(dirs, ["tauri"]);
    }

    #[test]
    fn deeper_paths_collapse_into_one_marker_per_folder() {
        let raw = b" M src\\a\\one.rs\0 M src\\a\\two.rs\0 M src\\b\\x.rs\0 M src\\Top.rs\0";
        let (direct, dirs) = split_direct(parse(raw, Some("src")));
        assert_eq!(direct.len(), 1);
        assert_eq!(dirs, ["a", "b"]);
    }

    #[test]
    fn clean_dirs_lists_tracked_folders_without_changes() {
        let tracked = vec![
            "src\\App.tsx".to_string(),
            "src\\deep\\x.rs".to_string(),
            "docs\\readme.md".to_string(),
            "README.md".to_string(),
            "dirty\\a.txt".to_string(),
        ];
        assert_eq!(clean_dirs(&tracked, None, &["dirty".to_string()]), ["docs", "src"]);
        // Inside a subfolder only that subtree counts, with the prefix stripped.
        assert_eq!(
            clean_dirs(&tracked, Some("src"), &[]),
            ["deep"]
        );
    }

    /// The regression this module was fixed for: opening a folder git had collapsed to
    /// one untracked record used to show no badges at all. Skipped when git is missing.
    #[test]
    fn status_inside_untracked_folder_lists_files() {
        let Some(git) = git_binary() else {
            return;
        };
        let root = std::env::temp_dir().join(format!("flowdir-git-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("newdir").join("deep")).unwrap();
        let run = |args: &[&str]| {
            let output = Command::new(git)
                .arg("-C")
                .arg(&root)
                .args(args)
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init", "-q"]);
        std::fs::write(root.join("tracked.txt"), "x").unwrap();
        run(&["add", "tracked.txt"]);
        run(&[
            "-c",
            "user.name=flowdir",
            "-c",
            "user.email=flowdir@example.com",
            "commit",
            "-qm",
            "init",
        ]);
        let clean = status(root.to_str().unwrap());
        assert!(!clean.dirty);
        assert!(clean.files.is_empty() && clean.dirs.is_empty());
        assert!(!clean.branch.is_empty());
        assert!(!clean.upstream && clean.ahead == 0 && clean.behind == 0);

        std::fs::write(root.join("newdir").join("a.txt"), "a").unwrap();
        std::fs::write(root.join("newdir").join("deep").join("b.txt"), "b").unwrap();
        // The watcher normally drops the cached status; the test writes directly.
        invalidate(root.to_str().unwrap());

        let inside = status(root.join("newdir").to_str().unwrap());
        assert!(inside.dirty);
        let pairs: Vec<(&str, &str)> = inside
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.code.as_str()))
            .collect();
        assert_eq!(pairs, [("a.txt", "U")]);
        assert_eq!(inside.dirs, ["deep"]);

        let top = status(root.to_str().unwrap());
        assert!(top.files.is_empty(), "{:?}", top.files.iter().map(|f| &f.path).collect::<Vec<_>>());
        assert_eq!(top.dirs, ["newdir"]);
        assert_eq!(top.branch, clean.branch);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn child_repos_marks_repository_folders_only() {
        let root = std::env::temp_dir().join(format!("flowdir-git-repos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo").join(".git")).unwrap();
        std::fs::create_dir_all(root.join("plain")).unwrap();
        std::fs::create_dir_all(root.join("worktree")).unwrap();
        std::fs::write(root.join("worktree").join(".git"), "gitdir: elsewhere\n").unwrap();
        std::fs::write(root.join("file.txt"), "x").unwrap();

        assert_eq!(child_repos(root.to_str().unwrap()), ["repo", "worktree"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unmerged_records_are_flagged() {
        assert!(has_conflict(b"UU merge.txt\0"));
        assert!(has_conflict(b"AA both-added.txt\0"));
        assert!(has_conflict(b"## main\0UD gone.txt\0"));
        assert!(!has_conflict(b"## main\0 M changed.txt\0?? new.txt\0"));
    }

    #[test]
    fn repo_states_summarises_each_child_repository() {
        let Some(git) = git_binary() else {
            return;
        };
        let root = std::env::temp_dir().join(format!("flowdir-git-states-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for name in ["clean", "dirty"] {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("tracked.txt"), "x").unwrap();
            for args in [
                vec!["init", "-q"],
                vec!["add", "tracked.txt"],
                vec![
                    "-c",
                    "user.name=flowdir",
                    "-c",
                    "user.email=flowdir@example.com",
                    "commit",
                    "-qm",
                    "init",
                ],
            ] {
                let output = Command::new(git)
                    .arg("-C")
                    .arg(&dir)
                    .args(&args)
                    .creation_flags(CREATE_NO_WINDOW)
                    .output()
                    .unwrap();
                assert!(output.status.success(), "git {args:?}");
            }
        }
        std::fs::write(root.join("dirty").join("untracked.txt"), "y").unwrap();

        let names = child_repos(root.to_str().unwrap());
        let repos = repo_states(git, root.to_str().unwrap(), names);
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].name, "clean");
        assert!(repos[0].checked && !repos[0].dirty && !repos[0].conflict);
        assert_eq!(repos[1].name, "dirty");
        assert!(repos[1].checked && repos[1].dirty);

        let _ = std::fs::remove_dir_all(&root);
    }
}
