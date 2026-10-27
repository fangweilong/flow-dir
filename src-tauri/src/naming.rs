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
pub fn unique(paths: &[String]) -> Vec<String> {
    let mut claimed = std::collections::HashSet::new();
    paths
        .iter()
        .map(|path| unique_one(path, &mut claimed))
        .collect()
}

const MAX_SUFFIX: u32 = 10_000;

fn unique_one(path: &str, claimed: &mut std::collections::HashSet<String>) -> String {
    if !is_taken(path, claimed) {
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
        if !is_taken(&candidate, claimed) {
            claimed.insert(candidate.to_lowercase());
            return candidate;
        }
    }
    path.to_string()
}

fn is_taken(path: &str, claimed: &std::collections::HashSet<String>) -> bool {
    claimed.contains(&path.to_lowercase()) || std::fs::symlink_metadata(PathBuf::from(path)).is_ok()
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
        let next = unique(&[taken.to_string_lossy().into_owned()]);
        assert_eq!(
            Path::new(&next[0]).file_name().unwrap().to_string_lossy(),
            "report (1).txt"
        );
        std::fs::write(Path::new(&next[0]), b"y").unwrap();
        let again = unique(&[taken.to_string_lossy().into_owned()]);
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
        assert_eq!(unique(&[free.clone()]), vec![free]);
        let bare = dir.join("notes");
        std::fs::write(&bare, b"x").unwrap();
        let next = unique(&[bare.to_string_lossy().into_owned()]);
        assert_eq!(
            Path::new(&next[0]).file_name().unwrap().to_string_lossy(),
            "notes (1)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unique_separates_paths_that_collide_with_each_other() {
        let dir = temp_dir("unique-batch");
        let target = dir.join("same.txt").to_string_lossy().into_owned();
        let names = unique(&[target.clone(), target]);
        assert_ne!(names[0].to_lowercase(), names[1].to_lowercase());
        assert!(names[0].ends_with("same.txt"));
        assert!(names[1].ends_with("same (1).txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
