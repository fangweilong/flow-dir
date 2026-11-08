//! Guard rails for operations that would remove or relocate critical directories.
//!
//! The shell performs the actual work, so the guard runs in `ops::validate` before any
//! item is queued. Both the GUI process and the elevated helper share it; the helper
//! receives the same rules through its pipe payload because it has no settings cache.
//!
//! Matching is deliberately narrow: a rule protects the configured directory itself and
//! every ancestor of it, but never its descendants. Blocking `%APPDATA%` must not stop a
//! user from deleting `%APPDATA%\SomeApp`; blocking `%USERPROFILE%` already covers every
//! profile ancestor such as `C:\Users`.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::GetLongPathNameW;

use crate::error::ApiError;
use crate::fs::expand_path;

/// Upper bound for the user-editable lists; keeps settings.toml and the per-operation
/// normalisation bounded even if the file is hand-edited.
pub const MAX_RULES: usize = 200;

/// User-configured path lists. `block` refuses the operation outright; `confirm` allows
/// it only when the UI collected a typed confirmation for the same sources.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Protection {
    pub block: Vec<String>,
    pub confirm: Vec<String>,
}

/// Checks `sources` against the two tiers before a delete, move or rename is queued.
///
/// A block match wins over a confirm match, and one blocked source rejects the whole
/// batch: partially applying a mixed selection is exactly the surprise this guard exists
/// to prevent. `confirmed` carries the paths the user explicitly confirmed; it is matched
/// by normalised path so a reordered or forward-slash spelling still counts.
pub fn check(
    sources: &[String],
    protection: &Protection,
    confirmed: &[String],
) -> Result<(), ApiError> {
    let block: Vec<String> = protection.block.iter().filter_map(|raw| rule_key(raw)).collect();
    let confirm: Vec<String> = protection.confirm.iter().filter_map(|raw| rule_key(raw)).collect();
    let confirmed: HashSet<String> = confirmed.iter().filter_map(|raw| key(raw)).collect();
    let mut refused = Vec::new();
    let mut pending = Vec::new();

    for source in sources {
        // `webdav://` is not a filesystem path: its runner (`webdav_ops`) rejects unsafe
        // child names, and no local protection rule can describe it.
        if crate::webdav::is_webdav(source) {
            continue;
        }
        // Shell parsing names are not always virtual: `shell:Startup` resolves to a real
        // folder, and skipping it entirely let a delete bypass every rule. Resolve to the
        // filesystem path when there is one; namespace-only items (recycle bin) have none
        // and keep their old behaviour.
        let resolved = if crate::shellfs::is_shell_path(source) {
            match crate::shellfs::filesystem_path(source) {
                Some(path) => Some(path),
                None => continue,
            }
        } else {
            None
        };
        let candidate = resolved.as_deref().unwrap_or(source);
        // Fail closed: a spelling that does not resolve to a filesystem location cannot be
        // proven safe, and silently skipping it was a bypass (relative paths, `...`).
        let Some(source_key) = key(candidate) else {
            refused.push(source.clone());
            continue;
        };
        if is_volume_root(&source_key) || block.iter().any(|rule| covers(&source_key, rule)) {
            refused.push(source.clone());
        } else if confirm.iter().any(|rule| covers(&source_key, rule))
            && !confirmed.contains(&source_key)
        {
            pending.push(source.clone());
        }
    }

    if !refused.is_empty() {
        return Err(ApiError::new("PROTECTED", refused.join("\n")));
    }
    if !pending.is_empty() {
        return Err(ApiError::new("NEEDS_CONFIRM", pending.join("\n")));
    }
    Ok(())
}

/// Expands environment variables and trims/dedupes a user-supplied rule list. The stored
/// form stays human-readable while matching still resolves `%VAR%` spellings.
pub fn normalize_rules(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in list {
        let text = expand_path(entry).trim().to_string();
        if text.is_empty() {
            continue;
        }
        if out.iter().any(|existing| existing.eq_ignore_ascii_case(&text)) {
            continue;
        }
        out.push(text);
        if out.len() == MAX_RULES {
            break;
        }
    }
    out
}

/// Comparison key for a path: absolute, backslash-separated, lexically normalised
/// (`..` resolved without touching the filesystem) and lowercased.
///
/// Returns `None` for relative or device paths that cannot name a filesystem location.
/// A literal `%` in a real file name must survive, so this does not expand variables;
/// rule strings go through `rule_key` first.
pub fn key(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let text = trimmed.replace('/', "\\");
    // Long-path and device prefixes carry no meaning for comparison, and `\\?\UNC\`
    // must become an ordinary UNC path before the share-root check sees it.
    let text = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_string()
    } else if let Some(rest) = text.strip_prefix(r"\\.\") {
        rest.to_string()
    } else {
        text
    };

    let (prefix, rest) = if let Some(rest) = text.strip_prefix(r"\\") {
        (r"\\".to_string(), rest)
    } else {
        let bytes = text.as_bytes();
        if bytes.len() < 2 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
            return None;
        }
        // `C:foo` is drive-relative: Win32 resolves it against the drive's current
        // directory, which this lexical key cannot know. Refuse to guess.
        if bytes.len() > 2 && bytes[2] != b'\\' {
            return None;
        }
        (text[..2].to_string(), &text[2..])
    };

    let mut parts: Vec<&str> = Vec::new();
    for part in rest.split('\\') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => {
                // Win32 strips trailing dots and spaces from each component before it
                // reaches the filesystem, so `C:\Windows.` and `C:\Windows ` name the
                // protected folder. Comparing the raw spelling would miss the block.
                let trimmed = other.trim_end_matches([' ', '.']);
                if trimmed.is_empty() {
                    // `...` / `   ` cannot name a file; refuse to guess at one.
                    return None;
                }
                parts.push(trimmed);
            }
        }
    }
    // `\\server` without a share is not a usable filesystem root.
    if prefix == "\\\\" && parts.len() < 2 {
        return None;
    }

    let mut out = prefix;
    for part in parts {
        // A UNC prefix already ends with a separator; adding another would produce
        // `\\\server\share`, which no longer looks like a share root.
        if !out.ends_with('\\') {
            out.push('\\');
        }
        out.push_str(part);
    }
    // Expand 8.3 aliases (`PROGRA~1`) before lowercasing; otherwise a short spelling
    // reaches the same folder while bypassing the rule.
    let out = resolve_short_name(&out).unwrap_or(out);
    // NTFS compares case-insensitively across the whole Unicode range, so ASCII-only
    // folding let a rule spelled with different non-ASCII casing (`Иван` vs `иван`) miss
    // the same folder.
    Some(out.to_lowercase())
}

/// `GetLongPathNameW` for paths that may contain an 8.3 component.
///
/// This runs for network paths too: `PROGRA~1` on a UNC share or mapped drive names the
/// same protected folder, and skipping the expansion let a short spelling bypass the rule.
/// The probe is still skipped unless the path contains `~`, so ordinary validation never
/// touches the network.
fn resolve_short_name(path: &str) -> Option<String> {
    if !path.contains('~') {
        return None;
    }
    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    let mut buf = vec![0u16; 512];
    loop {
        let len = unsafe { GetLongPathNameW(PCWSTR(wide.as_ptr()), Some(&mut buf)) };
        if len == 0 {
            return None;
        }
        let needed = len as usize;
        if needed < buf.len() {
            return Some(String::from_utf16_lossy(&buf[..needed]));
        }
        // The first call reports the required size (without the terminator).
        buf = vec![0u16; needed + 1];
    }
}

fn rule_key(raw: &str) -> Option<String> {
    key(&expand_path(raw))
}

/// A source covers a rule when it is the rule itself or one of its ancestors.
fn covers(source: &str, rule: &str) -> bool {
    source == rule
        || (rule.len() > source.len()
            && rule.starts_with(source)
            && rule.as_bytes().get(source.len()) == Some(&b'\\'))
}

/// Volume roots cannot be removed through the shell anyway, but rejecting them up front
/// keeps the user from deleting a whole drive through a path that happens to spell its
/// root (for example `C:`).
fn is_volume_root(key: &str) -> bool {
    let bytes = key.as_bytes();
    if bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return true;
    }
    if let Some(rest) = key.strip_prefix(r"\\") {
        let parts: Vec<&str> = rest.split('\\').collect();
        return parts.len() == 2 && parts.iter().all(|part| !part.is_empty());
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn protection(block: &[&str], confirm: &[&str]) -> Protection {
        Protection {
            block: block.iter().map(|value| value.to_string()).collect(),
            confirm: confirm.iter().map(|value| value.to_string()).collect(),
        }
    }

    fn sources(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn keys_resolve_dot_segments_and_windows_spellings() {
        assert_eq!(key(r"C:\Windows\"), key(r"c:/windows"));
        assert_eq!(key(r"\\?\C:\Windows"), key(r"C:\Windows"));
        assert_eq!(key(r"C:\Users\me\..\Public"), key(r"C:\Users\Public"));
        assert_eq!(key(r"\\?\UNC\server\share\dir"), key(r"\\server\share\dir"));
        assert_eq!(key(r"\\?\C:\"), key("C:"));
        assert_eq!(key("C:"), Some("c:".to_string()));
        assert_eq!(key("relative\\path"), None);
        assert_eq!(key(""), None);
        // Drive-relative spellings resolve against a per-drive CWD; refuse them.
        assert_eq!(key(r"C:AppData"), None);
        assert_eq!(key(r"C:"), Some("c:".to_string()));
        // Win32 strips trailing dots/spaces per component before touching the disk.
        assert_eq!(key(r"C:\Windows. "), key(r"C:\Windows"));
        assert_eq!(key(r"C:\Windows...\System32 "), key(r"C:\Windows\System32"));
        // A component that is only dots/spaces names nothing; refuse to guess.
        assert_eq!(key(r"C:\..."), None);
    }

    #[test]
    fn a_path_that_cannot_be_normalised_is_refused() {
        let rules = Protection::default();
        let error = check(&sources(&[r"relative\path"]), &rules, &[]).unwrap_err();
        assert_eq!(error.code, "PROTECTED");
        let error = check(&sources(&[r"C:\..."]), &rules, &[]).unwrap_err();
        assert_eq!(error.code, "PROTECTED");
    }

    #[test]
    fn virtual_addresses_are_not_local_protection_targets() {
        let rules = protection(&[r"C:\Windows"], &[]);
        // WebDAV and shell namespace paths have their own runners; the local guard must
        // not turn every remote delete into PROTECTED.
        assert!(check(&sources(&["webdav://nas/Photos"]), &rules, &[]).is_ok());
        assert!(check(&sources(&["shell:RecycleBinFolder"]), &rules, &[]).is_ok());
        assert!(check(&sources(&["::20D04FE0-3AEA-1069-A2D8-08002B30309D"]), &rules, &[]).is_ok());
        // A relative path is still refused.
        assert!(check(&sources(&["..\\Windows"]), &rules, &[]).is_err());
    }

    #[test]
    fn shell_aliases_are_checked_against_the_folder_they_name() {
        // `shell:Startup` resolves to a real folder; deleting it must hit the same rule
        // as the filesystem spelling. Skip when the test host cannot resolve the alias.
        let Some(startup) = crate::shellfs::filesystem_path("shell:Startup") else {
            return;
        };
        let rules = protection(&[&startup], &[]);
        let error = check(&sources(&["shell:Startup"]), &rules, &[]).unwrap_err();
        assert_eq!(error.code, "PROTECTED");
    }

    #[test]
    fn a_volume_root_is_always_blocked() {
        let rules = Protection::default();
        let error = check(&sources(&[r"C:\"]), &rules, &[]).unwrap_err();
        assert_eq!(error.code, "PROTECTED");
        let error = check(&sources(&[r"\\server\share"]), &rules, &[]).unwrap_err();
        assert_eq!(error.code, "PROTECTED");
        // A folder inside the share is a normal path.
        assert!(check(&sources(&[r"\\server\share\dir"]), &rules, &[]).is_ok());
    }

    #[test]
    fn block_matches_the_path_and_its_ancestors_only() {
        let rules = protection(&[r"C:\Users\me\AppData"], &[]);
        assert!(check(&sources(&[r"C:\Users\me\AppData"]), &rules, &[]).is_err());
        assert!(check(&sources(&[r"C:\Users\me"]), &rules, &[]).is_err());
        assert!(check(&sources(&["C:\\Users"]), &rules, &[]).is_err());
        // Descendants stay usable: cleanup tools routinely clear app caches here.
        assert!(check(&sources(&[r"C:\Users\me\AppData\Local\Temp"]), &rules, &[]).is_ok());
        // A name that only shares a prefix must not match.
        assert!(check(&sources(&[r"C:\Users\me\AppData2"]), &rules, &[]).is_ok());
    }

    #[test]
    fn confirm_matches_until_the_same_path_is_confirmed() {
        let rules = protection(&[], &[r"D:\Projects"]);
        let source = sources(&[r"D:\Projects"]);
        let error = check(&source, &rules, &[]).unwrap_err();
        assert_eq!(error.code, "NEEDS_CONFIRM");
        assert_eq!(error.detail, r"D:\Projects");
        assert!(check(&source, &rules, &sources(&[r"d:/projects"])).is_ok());
        // Confirming one path must not confirm a different one.
        assert!(check(&source, &rules, &sources(&[r"D:\Other"])).is_err());
    }

    #[test]
    fn block_wins_over_confirm() {
        let rules = protection(&[r"C:\Windows"], &[r"C:\Windows"]);
        let error = check(&sources(&[r"C:\Windows"]), &rules, &sources(&[r"C:\Windows"]))
            .unwrap_err();
        assert_eq!(error.code, "PROTECTED");
    }

    #[test]
    fn normalise_expands_and_dedupes_rules() {
        let expanded = normalize_rules(&[
            r"C:\Windows".to_string(),
            "  c:\\windows  ".to_string(),
            String::new(),
        ]);
        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0], r"C:\Windows");
    }
}
