//! Existence probes and "name (1)" conflict resolution for rename/move previews.
//!
//! The frontend cannot see the disk, so a batch rename that only compared names inside
//! the selection would silently overwrite an unselected file. These helpers answer from
//! the filesystem instead, and `symlink_metadata` keeps junctions from being followed
//! into their target.

use std::path::{Path, PathBuf};

/// True per path that currently exists on disk (case-insensitive, as Windows is).
pub fn existing(paths: &[String]) -> Vec<bool> {
    paths
        .iter()
        .map(|path| std::fs::symlink_metadata(path).is_ok())
        .collect()
}

/// First free variant of every path, inserting ` (n)` before the extension.
///
/// Paths are resolved in order and a variant is reserved as soon as it is picked, so two
/// inputs that would collide with each other still end up with distinct names. Names that
/// are already free are returned unchanged; the search stops after [`MAX_SUFFIX`] tries so
/// a pathological folder cannot hang the request.
///
/// `sources` lists the paths the same batch renames away, in execution order. Only the
/// sources already processed are treated as free: the shell applies queued renames
/// sequentially, so a later row's source still occupies its name when an earlier target
/// needs it. This keeps chains that run in order (`b`→`c`, then `a`→`b`) suffix-free
/// while a swap (`a`↔`b`) falls back to `b (1)` instead of producing a target that would
/// collide at execution time. A target that only changes the case of its own source is
/// covered by the same rule.
pub fn unique(paths: &[String], sources: &[String]) -> Vec<String> {
    let mut claimed = std::collections::HashSet::new();
    let mut released: std::collections::HashSet<String> = std::collections::HashSet::new();
    paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            // The current row's own source is free for a case-only rename.
            if let Some(source) = sources.get(index) {
                released.insert(source.to_lowercase());
            }
            unique_one(path, &released, &mut claimed)
        })
        .collect()
}

const MAX_SUFFIX: u32 = 10_000;

fn unique_one(
    path: &str,
    released: &std::collections::HashSet<String>,
    claimed: &mut std::collections::HashSet<String>,
) -> String {
    if !is_taken(path, released, claimed) {
        claimed.insert(path.to_lowercase());
        return path.to_string();
    }
    let original = Path::new(path);
    let dir = original.parent().map(Path::to_path_buf).unwrap_or_default();
    let name = original
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (stem, ext) = match name.rfind('.') {
        // A leading dot is part of the name, not an extension separator.
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name.as_str(), ""),
    };
    for n in 1..=MAX_SUFFIX {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        let candidate = candidate.to_string_lossy().into_owned();
        if !is_taken(&candidate, released, claimed) {
            claimed.insert(candidate.to_lowercase());
            return candidate;
        }
    }
    // Every sequential suffix is occupied (or reserved by this batch). Returning `path`
    // here would hand the caller a taken target, so fall back to a time/counter token.
    // The token changes every iteration, so this loop cannot return an occupied path.
    let mut token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0);
    loop {
        let candidate = dir.join(format!("{stem}~{token:016x}{ext}"));
        let candidate = candidate.to_string_lossy().into_owned();
        if !is_taken(&candidate, released, claimed) {
            claimed.insert(candidate.to_lowercase());
            return candidate;
        }
        token = token.wrapping_add(0x9E37_79B9_7F4A_7C15);
    }
}

fn is_taken(
    path: &str,
    released: &std::collections::HashSet<String>,
    claimed: &std::collections::HashSet<String>,
) -> bool {
    let key = path.to_lowercase();
    if claimed.contains(&key) {
        return true;
    }
    // A target reusing a path that an earlier row already vacated is intentional
    // (chain), not a conflict.
    if released.contains(&key) {
        return false;
    }
    std::fs::symlink_metadata(PathBuf::from(path)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "flowdir-naming-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn existing_reports_only_present_paths() {
        let dir = temp_dir("existing");
        let present = dir.join("a.txt");
        std::fs::write(&present, b"x").unwrap();
        let missing = dir.join("b.txt");
        let flags = existing(&[
            present.to_string_lossy().into_owned(),
            missing.to_string_lossy().into_owned(),
        ]);
        assert_eq!(flags, vec![true, false]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unique_inserts_a_counter_before_the_extension() {
        let dir = temp_dir("unique");
        let taken = dir.join("report.txt");
        std::fs::write(&taken, b"x").unwrap();
        let next = unique(&[taken.to_string_lossy().into_owned()], &[]);
        assert_eq!(
            Path::new(&next[0]).file_name().unwrap().to_string_lossy(),
            "report (1).txt"
        );
        std::fs::write(Path::new(&next[0]), b"y").unwrap();
        let again = unique(&[taken.to_string_lossy().into_owned()], &[]);
        assert_eq!(
            Path::new(&again[0]).file_name().unwrap().to_string_lossy(),
            "report (2).txt"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unique_keeps_free_paths_and_extensionless_names() {
        let dir = temp_dir("unique-free");
        let free = dir.join("free.txt").to_string_lossy().into_owned();
        assert_eq!(unique(std::slice::from_ref(&free), &[]), vec![free]);
        let bare = dir.join("notes");
        std::fs::write(&bare, b"x").unwrap();
        let next = unique(&[bare.to_string_lossy().into_owned()], &[]);
        assert_eq!(
            Path::new(&next[0]).file_name().unwrap().to_string_lossy(),
            "notes (1)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_case_only_target_is_not_suffixed() {
        let dir = temp_dir("unique-case");
        let source = dir.join("readme.md");
        std::fs::write(&source, b"x").unwrap();
        let from = source.to_string_lossy().into_owned();
        let to = dir.join("README.md").to_string_lossy().into_owned();
        // The target hits its own source on a case-insensitive volume, which is a rename,
        // not a collision. The frontend routes every row of a numbering batch through
        // `unique`, so without the hint a mixed batch renamed this row to "README (1).md".
        assert_eq!(unique(std::slice::from_ref(&to), std::slice::from_ref(&from)), vec![to]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unique_separates_paths_that_collide_with_each_other() {
        let dir = temp_dir("unique-batch");
        let target = dir.join("same.txt").to_string_lossy().into_owned();
        let names = unique(&[target.clone(), target], &[]);
        assert_ne!(names[0].to_lowercase(), names[1].to_lowercase());
        assert!(names[0].ends_with("same.txt"));
        assert!(names[1].ends_with("same (1).txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_batch_swap_is_numbered_because_the_shell_runs_in_order() {
        let dir = temp_dir("unique-swap");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        let from_a = a.to_string_lossy().into_owned();
        let from_b = b.to_string_lossy().into_owned();
        // a→b executes while b still exists, so `b` must be numbered; b→a then finds
        // `a` vacated by the first row.
        let names = unique(
            &[from_b.clone(), from_a.clone()],
            &[from_a.clone(), from_b.clone()],
        );
        assert_eq!(
            names,
            vec![
                dir.join("b (1).txt").to_string_lossy().into_owned(),
                from_a
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_chain_in_execution_order_keeps_the_target_free() {
        let dir = temp_dir("unique-chain");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        let from_a = a.to_string_lossy().into_owned();
        let from_b = b.to_string_lossy().into_owned();
        let free = dir.join("c.txt").to_string_lossy().into_owned();
        // b→c runs first, so a→b may reuse `b` without a suffix.
        let names = unique(&[free.clone(), from_b.clone()], &[from_b.clone(), from_a]);
        assert_eq!(names, vec![free, from_b]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
