//! Repository status for the file list (`git_status`).
//!
//! Shells out to the installed `git` binary instead of linking a second object model:
//! one porcelain run per folder is cheap, honours the user's config (fsmonitor,
//! large-repo settings, submodules) and needs no extra dependency.
//!
//! Listing never blocks on it: the command runs off the UI thread and the list paints
//! first, badges arrive afterwards.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use serde::Serialize;


const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Serialize, Clone)]
pub struct GitFile {
    /// Work-tree relative path with `\` separators, so the UI only joins.
    pub path: String,
    /// One display letter: M, A, D, R, C, T, U (untracked) or ! (conflict).
    pub code: String,
}

#[derive(Serialize, Clone, Default)]
pub struct GitStatus {
    /// Work-tree root; empty when the folder is not inside a repository.
    pub root: String,
    /// Changed entries that are direct children of the folder.
    pub files: Vec<GitFile>,
    /// Child folders holding changes deeper down; they only get a marker, not a letter.
    pub dirs: Vec<String>,
}

pub fn status(path: &str) -> GitStatus {
    let Some(git) = git_binary() else {
        return GitStatus::default();
    };
    let Some(root) = toplevel(git, path) else {
        return GitStatus::default();
    };
    let prefix = relative_to_root(&root, path);
    let mut command = Command::new(git);
    command
        .arg("--no-optional-locks")
        .args(["-c", "core.quotepath=false"])
        .arg("-C")
        .arg(&root)
        // `normal` collapses a wholly untracked folder into one `dir/` record, which hides
        // the status of every file inside it once opened. `all` names each file instead;
        // git still collapses foreign repositories, whose trailing separator marks them as
        // folders below.
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .creation_flags(CREATE_NO_WINDOW);
    let Some(output) = output_with_timeout(&mut command) else {
        return GitStatus {
            root,
            dirs: Vec::new(),
            files: Vec::new(),
        };
    };
    if !output.status.success() {
        return GitStatus {
            root,
            dirs: Vec::new(),
            files: Vec::new(),
        };
    }
    // Nested paths collapse to one marker per child folder: a work tree with 50k changes
    // below the listed folder must not push 50k strings through the IPC bridge.
    let (files, dirs) = split_direct(parse(&output.stdout, prefix.as_deref()));
    GitStatus { root, files, dirs }
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
        output_with_timeout(&mut command).is_some_and(|out| out.status.success())
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
    )?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let root = text.trim().trim_end_matches(['/', '\\']).replace('/', "\\");
    (!root.is_empty()).then_some(root)
}

/// Runs `command` with a hard deadline. A work tree on an unreachable share (or a stuck
/// `core.fsmonitor` hook) would otherwise pin a blocking thread forever; `None` means the
/// child was killed. Output is drained on threads because status on a large repository
/// writes far more than a pipe buffer.
fn output_with_timeout(command: &mut Command) -> Option<Output> {
    const TIMEOUT: Duration = Duration::from_secs(8);
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
    let deadline = Instant::now() + TIMEOUT;
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

/// Parses `--porcelain=v1 -z`: NUL separated `XY path` records, where renames and copies
/// carry the source path as one extra record.
///
/// `prefix` keeps only entries inside the requested folder and makes their paths relative
/// to it, so the reply stays small in repositories with thousands of changes.
fn parse(bytes: &[u8], prefix: Option<&str>) -> Vec<GitFile> {
    let mut files = Vec::new();
    let mut records = bytes.split(|byte| *byte == 0).filter(|r| !r.is_empty());
    while let Some(record) = records.next() {
        if record.len() < 4 {
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
        let raw = b" M src\\App.tsx\0?? new file.txt\0A  staged.txt\0 D gone.txt\0UU merge.txt\0R  renamed.txt\0old.txt\0 M dir\\nested.txt\0";
        let files = parse(raw, None);
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
        std::fs::write(root.join("newdir").join("a.txt"), "a").unwrap();
        std::fs::write(root.join("newdir").join("deep").join("b.txt"), "b").unwrap();

        let inside = status(root.join("newdir").to_str().unwrap());
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

        let _ = std::fs::remove_dir_all(&root);
    }
}
