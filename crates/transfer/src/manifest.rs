//! File-set manifest: what will be sent, and name sanitising on receipt.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::{TransferError, io_err};

/// Caps against hostile or accidental giant manifests.
pub const MAX_ENTRIES: usize = 500_000;
pub const MAX_DEPTH: usize = 64;
pub const MAX_NAME: usize = 255;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Path components relative to the set root (first = a top-level item).
    pub path: Vec<String>,
    pub size: u64,
    /// Modification time (Unix seconds), restored on the receiver.
    pub mtime: i64,
    pub is_dir: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub entries: Vec<Entry>,
    pub total_bytes: u64,
    /// The files were cut (move) rather than copied.
    pub cut: bool,
}

impl Manifest {
    /// Top-level item names (what the receiver's clipboard will list).
    #[must_use]
    pub fn roots(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for e in &self.entries {
            if let Some(first) = e.path.first()
                && !out.contains(first)
            {
                out.push(first.clone());
            }
        }
        out
    }

    /// Structural checks before anything touches the disk.
    pub fn validate(&self) -> Result<(), TransferError> {
        if self.entries.len() > MAX_ENTRIES {
            return Err(TransferError::BadManifest("too many entries"));
        }
        let mut sum = 0u64;
        for e in &self.entries {
            if e.path.is_empty() || e.path.len() > MAX_DEPTH {
                return Err(TransferError::BadManifest("bad path depth"));
            }
            if e.is_dir && e.size != 0 {
                return Err(TransferError::BadManifest("directory with a size"));
            }
            sum = sum.checked_add(e.size).ok_or(TransferError::BadManifest("size overflow"))?;
        }
        if sum != self.total_bytes {
            return Err(TransferError::BadManifest("sizes do not add up"));
        }
        Ok(())
    }
}

fn mtime(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

/// Walks the copied items (symlinks are not followed; they are skipped).
pub fn build_manifest(items: &[PathBuf], cut: bool) -> Result<Manifest, TransferError> {
    let mut entries = Vec::new();
    let mut total = 0u64;
    for item in items {
        let name = item
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or(TransferError::BadManifest("item without a name"))?;
        walk(item, vec![name], &mut entries, &mut total)?;
    }
    Ok(Manifest { entries, total_bytes: total, cut })
}

fn walk(path: &Path, rel: Vec<String>, out: &mut Vec<Entry>, total: &mut u64) -> Result<(), TransferError> {
    let meta = std::fs::symlink_metadata(path).map_err(io_err(path))?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if out.len() >= MAX_ENTRIES || rel.len() > MAX_DEPTH {
        return Err(TransferError::BadManifest("too many files or folders nested too deep"));
    }
    if meta.is_dir() {
        out.push(Entry { path: rel.clone(), size: 0, mtime: mtime(&meta), is_dir: true });
        let mut children: Vec<_> = std::fs::read_dir(path).map_err(io_err(path))?.filter_map(Result::ok).collect();
        children.sort_by_key(std::fs::DirEntry::file_name);
        for c in children {
            let mut r = rel.clone();
            r.push(c.file_name().to_string_lossy().into_owned());
            walk(&c.path(), r, out, total)?;
        }
    } else if meta.is_file() {
        *total = total.checked_add(meta.len()).ok_or(TransferError::BadManifest("size overflow"))?;
        out.push(Entry { path: rel, size: meta.len(), mtime: mtime(&meta), is_dir: false });
    }
    Ok(())
}

const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Makes one untrusted path component safe for this OS. Never returns
/// `.`/`..`, separators, control characters or an empty string.
#[must_use]
pub fn sanitize_component(name: &str) -> String {
    let bad = |c: char| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|');
    let mut s: String = name.chars().map(|c| if bad(c) { '_' } else { c }).collect();
    // Windows drops trailing dots/spaces, which could alias another name.
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    if s.is_empty() || s == "." || s == ".." {
        s = "_".into();
    }
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    if WINDOWS_RESERVED.contains(&stem.as_str()) {
        s.insert(0, '_');
    }
    if s.len() > MAX_NAME {
        let mut cut = MAX_NAME;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

/// Joins sanitised components under `root` (cannot escape it by construction).
#[must_use]
pub fn safe_join(root: &Path, components: &[String]) -> PathBuf {
    let mut p = root.to_path_buf();
    for c in components {
        p.push(sanitize_component(c));
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitises_hostile_names() {
        assert_eq!(sanitize_component(".."), "_");
        assert_eq!(sanitize_component("."), "_");
        assert_eq!(sanitize_component("a/../../etc"), "a_.._.._etc");
        assert_eq!(sanitize_component(r"C:\x"), "C__x");
        assert_eq!(sanitize_component("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_component("report. "), "report");
        assert_eq!(sanitize_component("tab\there"), "tab_here");
        assert_eq!(sanitize_component(""), "_");
        assert!(sanitize_component(&"é".repeat(300)).len() <= MAX_NAME);
    }

    #[test]
    fn safe_join_stays_inside_root() {
        let root = Path::new("/stage/set1");
        let p = safe_join(root, &["..".into(), "..".into(), "etc".into(), "passwd".into()]);
        assert!(p.starts_with(root), "{p:?}");
    }

    #[test]
    fn builds_manifest_for_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("docs");
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("a.txt"), b"hello").unwrap();
        std::fs::write(d.join("sub/b.bin"), vec![7u8; 1000]).unwrap();
        let f = tmp.path().join("top.txt");
        std::fs::write(&f, b"x").unwrap();
        let m = build_manifest(&[d, f], true).unwrap();
        assert_eq!(m.total_bytes, 1006);
        assert!(m.cut);
        assert_eq!(m.roots(), vec!["docs".to_string(), "top.txt".to_string()]);
        assert_eq!(m.entries.iter().filter(|e| e.is_dir).count(), 2);
        m.validate().unwrap();
    }

    #[test]
    fn validate_rejects_lies() {
        let mut m = Manifest {
            entries: vec![Entry { path: vec!["a".into()], size: 5, mtime: 0, is_dir: false }],
            total_bytes: 4,
            cut: false,
        };
        assert!(m.validate().is_err());
        m.total_bytes = 5;
        assert!(m.validate().is_ok());
        m.entries[0].path.clear();
        assert!(m.validate().is_err());
    }
}
