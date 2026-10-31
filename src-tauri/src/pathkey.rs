//! Path comparison keys shared by the git status cache and recycle-bin restore.
//!
//! Windows accepts the same folder written several ways (8.3 short names, forward
//! slashes, verbatim prefixes), and git or the shell may report their own spelling.
//! Both sides of a comparison go through [`path_key`] so they line up.

use std::path::Path;

/// Long, case-folded, `\`-separated form of `path`.
///
/// A missing leaf (a recycled file's original path) resolves through its existing parent;
/// that is exactly what restore needs to match the shell's displaced-from value.
pub fn path_key(path: &str) -> String {
    let raw = path.replace('/', "\\");
    let canonical = std::fs::canonicalize(&raw).ok().or_else(|| {
        let path = Path::new(&raw);
        let parent = path.parent()?.as_os_str().to_string_lossy().into_owned();
        let name = path.file_name()?.to_string_lossy().into_owned();
        Some(std::fs::canonicalize(parent).ok()?.join(name))
    });
    let resolved = canonical
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or(raw)
        .to_lowercase();
    let stripped = if let Some(unc) = resolved.strip_prefix(r"\\?\unc\") {
        format!(r"\\{unc}")
    } else {
        resolved.strip_prefix(r"\\?\").unwrap_or(&resolved).to_string()
    };
    // Keep `c:\` intact; every other key drops the trailing separator.
    let trimmed = stripped.trim_end_matches('\\');
    if trimmed.len() > 2 {
        trimmed.to_string()
    } else {
        stripped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetShortPathNameW;

    #[test]
    fn short_and_long_spellings_share_a_key() {
        let dir = std::env::temp_dir()
            .join(format!("flowdir-path-key-{}-abcdefghijklmnop", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let long = dir.to_string_lossy().into_owned();
        let wide: Vec<u16> = long.encode_utf16().chain(std::iter::once(0)).collect();

        let mut buf = vec![0u16; 1024];
        let len = unsafe { GetShortPathNameW(PCWSTR(wide.as_ptr()), Some(&mut buf)) };
        if len > 0 && (len as usize) <= buf.len() {
            let buf = &buf[..len as usize];
            let short = String::from_utf16_lossy(buf.strip_suffix(&[0]).unwrap_or(buf));
            if short != long {
                assert_eq!(path_key(&short), path_key(&long));
                assert_eq!(path_key(&short.to_uppercase()), path_key(&long));
                assert_eq!(
                    path_key(&long.replace('\\', "/")),
                    path_key(&long)
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_leaf_resolves_through_its_parent() {
        let dir = std::env::temp_dir().join(format!("flowdir-path-key-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("gone.txt");
        let key = path_key(&file.to_string_lossy());
        assert!(key.ends_with(r"\gone.txt"));
        assert_eq!(key, path_key(&dir.to_string_lossy()).to_string() + r"\gone.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }
}