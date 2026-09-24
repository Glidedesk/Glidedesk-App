//! System clipboard access (text, HTML, RTF, images, file lists) behind one
//! trait, plus portable format conversions (PLAN §6.3, §6.4).
//!
//! Sync model: the clipboard follows the cursor. When control moves to
//! another computer, the side that owns a newer clipboard sends it; nothing
//! is transferred while you stay on one screen.

pub mod convert;
pub mod mock;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

use std::path::PathBuf;

/// Clipboard contents we carry between machines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipData {
    pub text: Option<String>,
    /// HTML fragment (without the Windows `CF_HTML` envelope).
    pub html: Option<String>,
    pub rtf: Option<Vec<u8>>,
    pub png: Option<Vec<u8>>,
    /// Files and folders (copy or cut in Finder / Explorer).
    pub files: Vec<PathBuf>,
    /// Files were cut (Windows marks this; Finder has no cut).
    pub cut: bool,
}

impl ClipData {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.html.is_none() && self.rtf.is_none() && self.png.is_none() && self.files.is_empty()
    }

    /// Bytes of the non-file formats.
    #[must_use]
    pub fn byte_size(&self) -> u64 {
        let n = self.text.as_ref().map_or(0, String::len)
            + self.html.as_ref().map_or(0, String::len)
            + self.rtf.as_ref().map_or(0, Vec::len)
            + self.png.as_ref().map_or(0, Vec::len);
        n as u64
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClipError {
    #[error("clipboard is not available on this platform")]
    Unsupported,
    #[error("the clipboard is busy (another app has it open)")]
    Busy,
    #[error("clipboard error: {0}")]
    Os(String),
    #[error(transparent)]
    Convert(#[from] convert::ConvertError),
}

pub trait Clipboard: Send + std::fmt::Debug {
    /// Changes whenever any app writes the clipboard (cheap).
    fn sequence(&self) -> u64;
    /// Reads every supported format; formats larger than `limit` bytes are skipped (0 = no limit).
    fn read(&mut self, limit: u64) -> Result<ClipData, ClipError>;
    /// Replaces the clipboard contents.
    fn write(&mut self, data: &ClipData) -> Result<(), ClipError>;
}

/// The system clipboard.
pub fn system() -> Result<Box<dyn Clipboard>, ClipError> {
    #[cfg(target_os = "macos")]
    return Ok(Box::new(macos::MacClipboard::new()));
    #[cfg(windows)]
    return windows::WinClipboard::start().map(|c| Box::new(c) as Box<dyn Clipboard>);
    #[cfg(target_os = "linux")]
    return linux::LinuxClipboard::new().map(|c| Box::new(c) as Box<dyn Clipboard>);
    #[allow(unreachable_code)]
    Err(ClipError::Unsupported)
}

#[must_use]
#[cfg_attr(not(any(target_os = "macos", windows, target_os = "linux")), allow(dead_code))]
pub(crate) fn within(limit: u64, len: usize) -> bool {
    limit == 0 || len as u64 <= limit
}
