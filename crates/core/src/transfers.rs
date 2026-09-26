//! Registry of running and recent transfers, shown in the UI.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use glidedesk_ipc::views::{TransferState, TransferView};
use glidedesk_transfer::Progress;

const KEEP_FINISHED: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct Item {
    id: u64,
    peer: String,
    outgoing: bool,
    name: String,
    detail: Option<String>,
    progress: Arc<Progress>,
    state: TransferState,
    error: Option<String>,
    finished: Option<Instant>,
}

#[derive(Debug, Default)]
pub struct Transfers {
    items: Mutex<Vec<Item>>,
    next: std::sync::atomic::AtomicU64,
}

impl Transfers {
    pub fn start(&self, peer: &str, outgoing: bool, name: String, detail: Option<String>) -> (u64, Arc<Progress>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let progress = Arc::new(Progress::default());
        let mut items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        items.retain(|i| i.finished.is_none_or(|t| t.elapsed() < KEEP_FINISHED));
        items.push(Item {
            id,
            peer: peer.to_owned(),
            outgoing,
            name,
            detail,
            progress: progress.clone(),
            state: TransferState::Active,
            error: None,
            finished: None,
        });
        (id, progress)
    }

    pub fn finish(&self, id: u64, result: Result<(), String>) {
        let mut items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(i) = items.iter_mut().find(|i| i.id == id) {
            i.finished = Some(Instant::now());
            match result {
                Ok(()) => i.state = TransferState::Done,
                Err(e) => {
                    i.state = TransferState::Failed;
                    i.error = Some(e);
                }
            }
        }
    }

    /// Cancels every active transfer (clipboard replaced, link closed).
    pub fn cancel_all(&self) {
        for i in self.items.lock().unwrap_or_else(PoisonError::into_inner).iter() {
            if i.state == TransferState::Active {
                i.progress.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    #[must_use]
    pub fn any_active(&self) -> bool {
        self.items.lock().unwrap_or_else(PoisonError::into_inner).iter().any(|i| i.state == TransferState::Active)
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<TransferView> {
        let mut items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        items.retain(|i| i.finished.is_none_or(|t| t.elapsed() < KEEP_FINISHED));
        items
            .iter()
            .map(|i| TransferView {
                id: i.id,
                peer: i.peer.clone(),
                outgoing: i.outgoing,
                name: i.name.clone(),
                done: i.progress.done.load(Ordering::Relaxed),
                total: i.progress.total.load(Ordering::Relaxed),
                state: i.state,
                error: i.error.clone(),
                detail: i.detail.clone(),
            })
            .collect()
    }
}
