//! Private folder where received file sets are written before they are
//! placed on the clipboard. Old sets are cleaned up automatically.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::{TransferError, io_err};

#[derive(Clone, Debug)]
pub struct Staging {
    root: PathBuf,
}

impl Staging {
    /// Default: per-user cache folder `…/Nexpingdesk/Incoming`.
    pub fn default_location() -> Option<Self> {
        if let Some(home) = std::env::var_os("NEXPINGDESK_HOME").filter(|h| !h.is_empty()) {
            return Some(Self::at(PathBuf::from(home).join("incoming")));
        }
        let base = directories::BaseDirs::new()?;
        Some(Self::at(base.cache_dir().join("Nexpingdesk").join("Incoming")))
    }

    #[must_use]
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Creates a fresh, private folder for one set.
    pub fn new_set(&self, id: u64) -> Result<PathBuf, TransferError> {
        let dir = self.root.join(format!("{id:016x}"));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(io_err(&dir))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(io_err(&dir))?;
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(&dir).map_err(io_err(&dir))?;
        Ok(dir)
    }

    /// Free bytes on the disk that holds the staging folder (`None` if unknown).
    #[must_use]
    pub fn free_space(&self) -> Option<u64> {
        // The folder may not exist yet: ask for its nearest existing parent.
        let probe = self.root.ancestors().find(|p| p.exists())?;
        fs4::available_space(probe).ok()
    }

    /// Removes a set (after an error or when it is no longer needed).
    pub fn discard(&self, dir: &Path) {
        if dir.starts_with(&self.root) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// Deletes sets older than `age` (called at start-up and periodically).
    pub fn cleanup(&self, age: Duration) {
        let Ok(rd) = std::fs::read_dir(&self.root) else { return };
        let now = SystemTime::now();
        for e in rd.filter_map(Result::ok) {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|d| d > age);
            if old {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

/// Top-level items of a received set, as clipboard paths.
#[must_use]
pub fn roots_in(dir: &Path, roots: &[String]) -> Vec<PathBuf> {
    roots.iter().map(|r| crate::manifest::safe_join(dir, std::slice::from_ref(r))).filter(|p| p.exists()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_are_created_and_cleaned() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Staging::at(tmp.path().join("in"));
        let d = s.new_set(7).unwrap();
        std::fs::write(d.join("x"), b"1").unwrap();
        assert_eq!(roots_in(&d, &["x".into(), "missing".into()]), vec![d.join("x")]);
        s.cleanup(Duration::from_secs(3600));
        assert!(d.exists(), "fresh sets survive");
        s.cleanup(Duration::ZERO);
        assert!(!d.exists());
        s.discard(Path::new("/definitely/not/staging"));
    }
}
