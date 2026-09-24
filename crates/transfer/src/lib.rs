//! File transfer for copied files (PLAN §6.4).
//!
//! A *file set* is what was copied in Finder / Explorer: files and folders.
//! The sender streams a manifest followed by every file's bytes and a BLAKE3
//! hash; the receiver writes into a private staging folder, verifies each
//! file, and only then exposes the set (so a paste never sees partial data).
//!
//! Everything from the peer is untrusted: names are sanitised for the local
//! OS, paths can never escape the staging folder, and counts/sizes are capped.

pub mod manifest;
pub mod staging;
pub mod stream;

pub use manifest::{Entry, Manifest, build_manifest, sanitize_component};
pub use staging::Staging;
pub use stream::{Progress, receive_set, send_set};

#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    #[error("I/O error on {path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error("network: {0}")]
    Net(String),
    #[error("the other computer sent an invalid file list: {0}")]
    BadManifest(&'static str),
    #[error("file '{0}' is corrupted (checksum mismatch)")]
    Corrupt(String),
    #[error("transfer cancelled")]
    Cancelled,
    #[error("files changed while copying: {0}")]
    Changed(String),
}

pub(crate) fn io_err(path: &std::path::Path) -> impl FnOnce(std::io::Error) -> TransferError + '_ {
    move |source| TransferError::Io { path: path.display().to_string(), source }
}
