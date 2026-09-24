//! Clipboard and file sync (PLAN §6.3, §6.4): the clipboard follows the
//! cursor. When control moves to a machine, the side with a newer clipboard
//! sends it on its own QUIC stream, so input is never delayed.
//!
//! Stream formats (after the 1-byte kind):
//! * clipboard: `u32 len + postcard ClipHeader`, then every part's bytes.
//! * files: `u64 set id`, then `glidedesk_transfer::send_set` data.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use glidedesk_clipboard::{ClipData, Clipboard};
use glidedesk_proto::{ClipFormat, ClipHeader, DeviceId, FileSetId, MAX_CLIPBOARD_BYTES, stream_kind};
use glidedesk_transfer::{Manifest, Staging, build_manifest, receive_set, send_set};
use tracing::{debug, info, warn};

use crate::transfers::Transfers;

const MAX_HEADER: usize = 64 * 1024;
/// Transfers smaller than this are not listed in the UI.
const LIST_THRESHOLD: u64 = 1 << 20;
const TAKEN_WATCH: Duration = Duration::from_secs(24 * 3600);

type SharedClip = Arc<Mutex<Box<dyn Clipboard>>>;
/// Recipient, original paths and what was sent.
type CutSet = (Option<DeviceId>, Vec<PathBuf>, Manifest);

/// What a received file set needs for cut & paste bookkeeping.
#[derive(Debug)]
pub struct ReceivedFiles {
    pub set: FileSetId,
    pub cut: bool,
    pub roots: Vec<PathBuf>,
}

pub struct Sync {
    clip: Option<SharedClip>,
    /// Clipboard sequence right after our own last write, and who sent it.
    written: Mutex<Option<(u64, Option<DeviceId>)>>,
    staging: Option<Staging>,
    pub transfers: Arc<Transfers>,
    /// Sets we sent as "cut": recipient, originals and manifest, until that
    /// recipient reports it moved them. Only the recipient can release a set.
    cuts: Mutex<HashMap<u64, CutSet>>,
    next_set: AtomicU64,
}

impl std::fmt::Debug for Sync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sync").field("clipboard", &self.clip.is_some()).finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn describe(m: &Manifest) -> String {
    let roots = m.roots();
    match roots.as_slice() {
        [one] => one.clone(),
        many => format!("{} items", many.len()),
    }
}

impl Sync {
    #[must_use]
    pub fn new(clip: Option<Box<dyn Clipboard>>) -> Arc<Self> {
        let staging = Staging::default_location();
        if let Some(s) = &staging {
            s.cleanup(Duration::from_secs(24 * 3600));
        }
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| u64::try_from(d.as_nanos() & u128::from(u64::MAX)).unwrap_or(1));
        Arc::new(Self {
            clip: clip.map(|c| Arc::new(Mutex::new(c))),
            written: Mutex::new(None),
            staging,
            transfers: Arc::new(Transfers::default()),
            cuts: Mutex::new(HashMap::new()),
            next_set: AtomicU64::new(seed),
        })
    }

    #[must_use]
    pub fn available(&self) -> bool {
        self.clip.is_some()
    }

    /// Current clipboard sequence if it holds something `target` has not seen.
    /// `target` = the machine we would send to (`None` = the server).
    #[must_use]
    pub fn changed_for(&self, target: Option<DeviceId>, last_sent: Option<u64>) -> Option<u64> {
        let seq = lock(self.clip.as_ref()?).sequence();
        if Some(seq) == last_sent {
            return None;
        }
        // Do not echo back what that machine just gave us.
        if *lock(&self.written) == Some((seq, target)) {
            return None;
        }
        Some(seq)
    }

    /// Reads the local clipboard and sends it (text/images or files).
    /// `target` is the machine receiving (`None` = the server).
    #[allow(clippy::too_many_arguments)]
    pub async fn send(
        self: Arc<Self>,
        conn: quinn::Connection,
        target: Option<DeviceId>,
        peer: String,
        allow_clip: bool,
        allow_files: bool,
        limit: u64,
    ) -> Result<(), String> {
        let Some(clip) = self.clip.clone() else { return Ok(()) };
        let cap = if limit == 0 { MAX_CLIPBOARD_BYTES } else { limit.min(MAX_CLIPBOARD_BYTES) };
        let data = tokio::task::spawn_blocking(move || lock(&clip).read(cap))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        if !data.files.is_empty() {
            if allow_files {
                return self.send_files(conn, target, peer, data.files, data.cut).await;
            }
            return Ok(());
        }
        if !allow_clip || data.is_empty() {
            return Ok(());
        }
        let mut parts: Vec<(ClipFormat, Vec<u8>)> = Vec::new();
        if let Some(t) = data.text {
            parts.push((ClipFormat::Text, t.into_bytes()));
        }
        if let Some(h) = data.html {
            parts.push((ClipFormat::Html, h.into_bytes()));
        }
        if let Some(r) = data.rtf {
            parts.push((ClipFormat::Rtf, r));
        }
        if let Some(p) = data.png {
            parts.push((ClipFormat::Png, p));
        }
        let header = ClipHeader { parts: parts.iter().map(|(f, b)| (*f, b.len() as u64)).collect(), files: None };
        let total: u64 = header.parts.iter().map(|(_, n)| n).sum();
        let tracked = (total >= LIST_THRESHOLD).then(|| self.transfers.start(&peer, true, "Clipboard".into()));
        let res = async {
            let body = postcard::to_stdvec(&header).map_err(|e| e.to_string())?;
            let mut s = conn.open_uni().await.map_err(|e| e.to_string())?;
            s.write_all(&[stream_kind::CLIPBOARD]).await.map_err(|e| e.to_string())?;
            s.write_all(&u32::try_from(body.len()).unwrap_or(0).to_le_bytes()).await.map_err(|e| e.to_string())?;
            s.write_all(&body).await.map_err(|e| e.to_string())?;
            for (_, bytes) in &parts {
                s.write_all(bytes).await.map_err(|e| e.to_string())?;
                if let Some((_, p)) = &tracked {
                    p.done.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                }
            }
            s.finish().map_err(|e| e.to_string())?;
            Ok::<(), String>(())
        }
        .await;
        if let Some((id, p)) = tracked {
            p.total.store(total, Ordering::Relaxed);
            self.transfers.finish(id, res.clone());
        }
        debug!(bytes = total, "clipboard sent");
        res
    }

    async fn send_files(
        self: Arc<Self>,
        conn: quinn::Connection,
        target: Option<DeviceId>,
        peer: String,
        files: Vec<PathBuf>,
        cut: bool,
    ) -> Result<(), String> {
        let list = files.clone();
        let manifest = tokio::task::spawn_blocking(move || build_manifest(&list, cut))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let set = random_id();
        if cut {
            lock(&self.cuts).insert(set, (target, files.clone(), manifest.clone()));
        }
        let (tid, progress) = self.transfers.start(&peer, true, describe(&manifest));
        let res = async {
            let mut s = conn.open_uni().await.map_err(|e| e.to_string())?;
            s.write_all(&[stream_kind::FILES]).await.map_err(|e| e.to_string())?;
            s.write_all(&set.to_le_bytes()).await.map_err(|e| e.to_string())?;
            send_set(&mut s, &files, &manifest, &progress).await.map_err(|e| e.to_string())?;
            s.finish().map_err(|e| e.to_string())?;
            Ok::<(), String>(())
        }
        .await;
        if res.is_err() {
            lock(&self.cuts).remove(&set);
        }
        info!(bytes = manifest.total_bytes, ok = res.is_ok(), "files sent");
        self.transfers.finish(tid, res.clone());
        res
    }

    /// Handles one incoming clipboard/files stream (kind byte already read).
    #[allow(clippy::too_many_arguments)]
    pub async fn receive(
        self: Arc<Self>,
        kind: u8,
        mut stream: quinn::RecvStream,
        origin: Option<DeviceId>,
        peer: String,
        allow_clip: bool,
        allow_files: bool,
        limit: u64,
    ) -> Result<Option<ReceivedFiles>, String> {
        let Some(clip) = self.clip.clone() else {
            let _ = stream.stop(0u32.into());
            return Ok(None);
        };
        match kind {
            stream_kind::CLIPBOARD if allow_clip => {
                let data = read_clip(&mut stream, limit).await?;
                self.write_local(&clip, data, origin).await?;
                Ok(None)
            }
            stream_kind::FILES if allow_files => {
                let Some(staging) = self.staging.clone() else { return Err("no staging folder".into()) };
                let mut id = [0u8; 8];
                stream.read_exact(&mut id).await.map_err(|e| e.to_string())?;
                let set = FileSetId(u64::from_le_bytes(id));
                let dir = staging.new_set(self.next_set.fetch_add(1, Ordering::Relaxed)).map_err(|e| e.to_string())?;
                let (tid, progress) = self.transfers.start(&peer, false, "Files".into());
                let res = receive_set(&mut stream, &dir, 0, progress).await;
                match res {
                    Ok(manifest) => {
                        let roots = glidedesk_transfer::staging::roots_in(&dir, &manifest.roots());
                        let data = ClipData { files: roots.clone(), cut: manifest.cut, ..ClipData::default() };
                        self.write_local(&clip, data, origin).await?;
                        self.transfers.finish(tid, Ok(()));
                        info!(bytes = manifest.total_bytes, "files received");
                        Ok(Some(ReceivedFiles { set, cut: manifest.cut, roots }))
                    }
                    Err(e) => {
                        staging.discard(&dir);
                        self.transfers.finish(tid, Err(e.to_string()));
                        Err(e.to_string())
                    }
                }
            }
            _ => {
                // Not allowed or unknown: refuse without reading.
                let _ = stream.stop(0u32.into());
                Ok(None)
            }
        }
    }

    async fn write_local(&self, clip: &SharedClip, data: ClipData, origin: Option<DeviceId>) -> Result<(), String> {
        let clip = clip.clone();
        let seq = tokio::task::spawn_blocking(move || {
            let mut c = lock(&clip);
            c.write(&data).map(|()| c.sequence())
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        *lock(&self.written) = Some((seq, origin));
        Ok(())
    }

    /// The receiver moved our cut files: move the originals to the Trash,
    /// but only if they are exactly what was sent.
    /// `from` is the machine that sent `FilesTaken`; it must be the one the set went to.
    pub async fn files_taken(self: Arc<Self>, set: FileSetId, from: Option<DeviceId>) {
        let originals_sent = {
            let mut cuts = lock(&self.cuts);
            match cuts.get(&set.0) {
                Some((to, _, _)) if *to == from => cuts.remove(&set.0).map(|(_, o, m)| (o, m)),
                Some(_) => {
                    warn!(set = set.0, "ignored FilesTaken from a computer the files were not sent to");
                    None
                }
                None => None,
            }
        };
        let Some((originals, sent)) = originals_sent else { return };
        let res = tokio::task::spawn_blocking(move || {
            let now = build_manifest(&originals, true).map_err(|e| e.to_string())?;
            if now.entries != sent.entries {
                return Err("the files changed after they were copied; originals kept".to_owned());
            }
            for p in &originals {
                glidedesk_platform::move_to_trash(p)?;
            }
            Ok(originals.len())
        })
        .await;
        match res {
            Ok(Ok(n)) => info!(items = n, "cut files moved to the trash after the paste"),
            Ok(Err(e)) => warn!(error = %e, "cut: originals not removed"),
            Err(e) => warn!(error = %e, "cut task failed"),
        }
    }
}

/// Unguessable 64-bit id (file-set ids must not be predictable by other peers).
fn random_id() -> u64 {
    let mut b = [0u8; 8];
    if getrandom::fill(&mut b).is_err() {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        b[..4].copy_from_slice(&t.to_le_bytes());
    }
    u64::from_le_bytes(b)
}

/// Waits until the user moved all `roots` out of staging (cut & paste).
/// Returns `false` after the watch window expires.
pub async fn wait_taken(roots: Vec<PathBuf>) -> bool {
    let deadline = tokio::time::Instant::now() + TAKEN_WATCH;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if !roots.is_empty() && roots.iter().all(|r| !r.exists()) {
            return true;
        }
    }
    false
}

async fn read_clip(stream: &mut quinn::RecvStream, limit: u64) -> Result<ClipData, String> {
    let cap = if limit == 0 { MAX_CLIPBOARD_BYTES } else { limit.min(MAX_CLIPBOARD_BYTES) };
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).await.map_err(|e| e.to_string())?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_HEADER {
        return Err("clipboard header too large".into());
    }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).await.map_err(|e| e.to_string())?;
    let header: ClipHeader = postcard::from_bytes(&body).map_err(|_| "bad clipboard header".to_owned())?;
    if header.parts.len() > 8 {
        return Err("too many clipboard parts".into());
    }
    let total: u64 = header.parts.iter().map(|(_, n)| *n).try_fold(0u64, u64::checked_add).ok_or("size overflow")?;
    if total > cap {
        return Err(format!("clipboard of {total} bytes is over the limit"));
    }
    let mut out = ClipData::default();
    for (fmt, n) in header.parts {
        let mut buf = vec![0u8; usize::try_from(n).map_err(|_| "size")?];
        stream.read_exact(&mut buf).await.map_err(|e| e.to_string())?;
        match fmt {
            ClipFormat::Text => out.text = Some(String::from_utf8_lossy(&buf).into_owned()),
            ClipFormat::Html => out.html = Some(String::from_utf8_lossy(&buf).into_owned()),
            ClipFormat::Rtf => out.rtf = Some(buf),
            ClipFormat::Png => out.png = Some(buf),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_the_recipient_can_release_a_cut_set() {
        let sync = Sync::new(None);
        let (a, b) = (DeviceId([1; 16]), DeviceId([2; 16]));
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("keep.txt");
        std::fs::write(&f, b"data").unwrap();
        let m = build_manifest(std::slice::from_ref(&f), true).unwrap();
        lock(&sync.cuts).insert(42, (Some(a), vec![f.clone()], m));
        sync.clone().files_taken(FileSetId(42), Some(b)).await;
        assert!(lock(&sync.cuts).contains_key(&42), "another computer must not release the set");
        assert!(f.exists());
        sync.clone().files_taken(FileSetId(42), Some(a)).await;
        assert!(!lock(&sync.cuts).contains_key(&42), "the recipient releases it");
    }

    #[test]
    fn set_ids_are_not_sequential() {
        let (x, y) = (random_id(), random_id());
        assert_ne!(x.wrapping_add(1), y);
    }
}
