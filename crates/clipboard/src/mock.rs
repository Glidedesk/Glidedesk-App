//! In-memory clipboard for tests.

use std::sync::{Arc, Mutex, PoisonError};

use crate::{ClipData, ClipError, Clipboard};

#[derive(Clone, Debug, Default)]
pub struct MockClipboard {
    inner: Arc<Mutex<(u64, ClipData)>>,
}

impl MockClipboard {
    /// Simulates another app copying something.
    pub fn set_external(&self, data: ClipData) {
        let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        g.0 += 1;
        g.1 = data;
    }

    #[must_use]
    pub fn contents(&self) -> ClipData {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner).1.clone()
    }
}

impl Clipboard for MockClipboard {
    fn sequence(&self) -> u64 {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner).0
    }
    fn read(&mut self, _limit: u64) -> Result<ClipData, ClipError> {
        Ok(self.contents())
    }
    fn write(&mut self, data: &ClipData) -> Result<(), ClipError> {
        self.set_external(data.clone());
        Ok(())
    }
}
