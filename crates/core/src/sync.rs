//! Clipboard and file sync (PLAN §6.3, §6.4): the clipboard follows the
//! cursor. When control moves to a machine, the side with a newer clipboard
//! sends it on its own QUIC stream, so input is never delayed.
//!
//! Stream formats (after the 1-byte kind):
//! * clipboard: `u32 len + postcard ClipHeader`, then every part's bytes.
//! * offer (uni): `u32 len + postcard FileOffer` — copied files, no data (§14.2).
//! * fetch (bi, opened by the receiver on paste): `u64 set id` out,
//!   `glidedesk_transfer::send_set` data back.
//!
//! Files never move by themselves: the receiver's paste shortcut is held,
//! the set is fetched into staging, put on the clipboard, and the paste is
//! replayed so the file manager copies it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use glidedesk_clipboard::{ClipData, Clipboard};
use glidedesk_proto::{ClipFormat, ClipHeader, DeviceId, FileOffer, FileSetId, MAX_CLIPBOARD_BYTES, stream_kind};
use glidedesk_transfer::{Manifest, Staging, build_manifest, receive_set, send_set};
use tokio::sync::watch;
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
    /// Who offered them (`None` = the server).
    pub origin: Option<DeviceId>,
}

/// Result of handling one incoming stream.
#[derive(Debug)]
pub enum Received {
    Nothing,
    /// Files were offered and wait for a paste here.
    Offer,
}

/// How many recent offers we keep ready to be fetched.
const MAX_OFFERS: usize = 8;
const OFFER_TTL: Duration = Duration::from_secs(3600);

/// Files we offered: who may fetch them, and what exactly.
struct Offered {
    set: u64,
    target: Option<DeviceId>,
    files: Vec<PathBuf>,
    manifest: Manifest,
    at: std::time::Instant,
}

/// An offer made to us, waiting for a paste.
struct Pending {
    offer: FileOffer,
    conn: quinn::Connection,
    origin: Option<DeviceId>,
    peer: String,
    /// Our clipboard sequence right after writing the placeholder: if it
    /// changes, the user copied something else and the offer is stale.
    placeholder: u64,
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
    offers: Mutex<Vec<Offered>>,
    pending: Mutex<Option<Pending>>,
    /// `true` while an offer waits here (the capture holds the paste shortcut).
    hold: watch::Sender<bool>,
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
            offers: Mutex::new(Vec::new()),
            pending: Mutex::new(None),
            hold: watch::channel(false).0,
        })
    }

    /// Follows "an offer waits for a paste here".
    #[must_use]
    pub fn hold_watch(&self) -> watch::Receiver<bool> {
        self.hold.subscribe()
    }

    /// "report.pdf from Studio-Mac" while an offer waits here.
    #[must_use]
    pub fn offer_description(&self) -> Option<String> {
        if !self.offer_pending() {
            return None;
        }
        let pending = lock(&self.pending);
        pending.as_ref().map(|p| format!("{} from {}", describe_names(&p.offer.names, p.offer.items), p.peer))
    }

    /// Files were offered to this computer and the clipboard still shows them.
    #[must_use]
    pub fn offer_pending(&self) -> bool {
        let Some(clip) = &self.clip else { return false };
        let placeholder = lock(&self.pending).as_ref().map(|p| p.placeholder);
        placeholder.is_some_and(|seq| lock(clip).sequence() == seq)
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
        if self.offer_pending() {
            // Our clipboard only shows files offered by another computer and
            // the cursor moves on to a third one: fetch them here first, so
            // they can be offered onwards.
            if !allow_files {
                return Ok(());
            }
            self.clone().fetch_offer().await?;
        }
        let cap = if limit == 0 { MAX_CLIPBOARD_BYTES } else { limit.min(MAX_CLIPBOARD_BYTES) };
        let data = tokio::task::spawn_blocking(move || lock(&clip).read(cap))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        if !data.files.is_empty() {
            if allow_files {
                return self.offer(conn, target, data.files, data.cut).await;
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

    /// Offers copied files to `target`: names and sizes only (§14.2).
    async fn offer(
        self: Arc<Self>,
        conn: quinn::Connection,
        target: Option<DeviceId>,
        files: Vec<PathBuf>,
        cut: bool,
    ) -> Result<(), String> {
        let list = files.clone();
        let manifest = tokio::task::spawn_blocking(move || build_manifest(&list, cut))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let set = random_id();
        let roots = manifest.roots();
        let offer = FileOffer {
            set: FileSetId(set),
            names: roots.iter().take(FileOffer::MAX_NAMES).cloned().collect(),
            items: u32::try_from(roots.len()).unwrap_or(u32::MAX),
            total_bytes: manifest.total_bytes,
            cut,
        };
        if cut {
            lock(&self.cuts).insert(set, (target, files.clone(), manifest.clone()));
        }
        {
            let mut offers = lock(&self.offers);
            offers.retain(|o| o.at.elapsed() < OFFER_TTL && o.target != target);
            if offers.len() >= MAX_OFFERS {
                offers.remove(0);
            }
            offers.push(Offered { set, target, files, manifest, at: std::time::Instant::now() });
        }
        let body = postcard::to_stdvec(&offer).map_err(|e| e.to_string())?;
        let mut s = conn.open_uni().await.map_err(|e| e.to_string())?;
        s.write_all(&[stream_kind::OFFER]).await.map_err(|e| e.to_string())?;
        s.write_all(&u32::try_from(body.len()).unwrap_or(0).to_le_bytes()).await.map_err(|e| e.to_string())?;
        s.write_all(&body).await.map_err(|e| e.to_string())?;
        s.finish().map_err(|e| e.to_string())?;
        info!(items = offer.items, bytes = offer.total_bytes, "files offered");
        Ok(())
    }

    /// Serves a fetch the peer opened after the user pasted there (kind byte
    /// already read). Only a set offered *to that peer* is sent.
    pub async fn serve_fetch(
        self: Arc<Self>,
        mut send: quinn::SendStream,
        mut recv: quinn::RecvStream,
        from: Option<DeviceId>,
        peer: String,
    ) {
        let mut id = [0u8; 8];
        if tokio::time::timeout(Duration::from_secs(10), recv.read_exact(&mut id)).await.map_or(true, |r| r.is_err()) {
            let _ = send.reset(0u32.into());
            return;
        }
        let set = u64::from_le_bytes(id);
        let found = {
            let mut offers = lock(&self.offers);
            offers.retain(|o| o.at.elapsed() < OFFER_TTL);
            offers.iter().find(|o| o.set == set && o.target == from).map(|o| (o.files.clone(), o.manifest.clone()))
        };
        let Some((files, manifest)) = found else {
            warn!(set, "fetch for files that were not offered to that computer");
            let _ = send.reset(0u32.into());
            return;
        };
        let (tid, progress) = self.transfers.start(&peer, true, describe(&manifest));
        let res = async {
            send_set(&mut send, &files, &manifest, &progress).await.map_err(|e| e.to_string())?;
            send.finish().map_err(|e| e.to_string())?;
            Ok::<(), String>(())
        }
        .await;
        info!(bytes = manifest.total_bytes, ok = res.is_ok(), "offered files sent");
        self.transfers.finish(tid, res);
    }

    /// Fetches the offered files into staging and puts them on the clipboard.
    /// `Ok(None)`: nothing (or nothing current) was offered.
    pub async fn fetch_offer(self: Arc<Self>) -> Result<Option<ReceivedFiles>, String> {
        let Some(clip) = self.clip.clone() else { return Ok(None) };
        let pending = lock(&self.pending).take();
        self.hold.send_replace(false);
        let Some(p) = pending else { return Ok(None) };
        if lock(&clip).sequence() != p.placeholder {
            return Ok(None); // the user copied something else since
        }
        let staging = self.staging.clone().ok_or("no staging folder")?;
        if let Some(free) = staging.free_space()
            && free < p.offer.total_bytes.saturating_add(64 << 20)
        {
            return Err(format!(
                "not enough free disk space for {} ({} free)",
                human(p.offer.total_bytes),
                human(free)
            ));
        }
        let name = describe_names(&p.offer.names, p.offer.items);
        let (tid, progress) = self.transfers.start(&p.peer, false, name.clone());
        progress.total.store(p.offer.total_bytes, Ordering::Relaxed);
        let dir = staging.new_set(self.next_set.fetch_add(1, Ordering::Relaxed)).map_err(|e| e.to_string())?;
        let res = async {
            let (mut send, mut recv) = p.conn.open_bi().await.map_err(|e| e.to_string())?;
            send.write_all(&[stream_kind::FETCH]).await.map_err(|e| e.to_string())?;
            send.write_all(&p.offer.set.0.to_le_bytes()).await.map_err(|e| e.to_string())?;
            let _ = send.finish();
            // The sender may not send more than it offered. `0` means "no limit"
            // to `receive_set`, so an empty offer may send at most 1 byte.
            let manifest = receive_set(&mut recv, &dir, p.offer.total_bytes.max(1), progress.clone()).await.map_err(
                |e| match e {
                    glidedesk_transfer::TransferError::Net(_) => {
                        format!("{name} is no longer available on {}", p.peer)
                    }
                    other => other.to_string(),
                },
            )?;
            Ok::<Manifest, String>(manifest)
        }
        .await;
        let manifest = match res {
            Ok(m) => m,
            Err(e) => {
                staging.discard(&dir);
                self.transfers.finish(tid, Err(e.clone()));
                return Err(e);
            }
        };
        if lock(&clip).sequence() != p.placeholder {
            staging.discard(&dir);
            self.transfers.finish(tid, Err("cancelled: something else was copied".into()));
            return Ok(None);
        }
        let roots = glidedesk_transfer::staging::roots_in(&dir, &manifest.roots());
        let cut = p.offer.cut;
        let data = ClipData { files: roots.clone(), cut, ..ClipData::default() };
        self.write_local(&clip, data, p.origin).await?;
        self.transfers.finish(tid, Ok(()));
        info!(bytes = manifest.total_bytes, "offered files received");
        Ok(Some(ReceivedFiles { set: p.offer.set, cut, roots, origin: p.origin }))
    }

    /// Handles one incoming clipboard/offer stream (kind byte already read).
    /// `conn` is the link it came on (used to fetch offered files later).
    #[allow(clippy::too_many_arguments)]
    pub async fn receive(
        self: Arc<Self>,
        kind: u8,
        mut stream: quinn::RecvStream,
        conn: quinn::Connection,
        origin: Option<DeviceId>,
        peer: String,
        allow_clip: bool,
        allow_files: bool,
        limit: u64,
    ) -> Result<Received, String> {
        let Some(clip) = self.clip.clone() else {
            let _ = stream.stop(0u32.into());
            return Ok(Received::Nothing);
        };
        match kind {
            stream_kind::CLIPBOARD if allow_clip => {
                let data = read_clip(&mut stream, limit).await?;
                self.write_local(&clip, data, origin).await?;
                self.drop_pending();
                Ok(Received::Nothing)
            }
            stream_kind::OFFER if allow_files => {
                let offer = read_offer(&mut stream).await?;
                let text = format!(
                    "{} from {peer} — paste with the keyboard shortcut to copy {} here",
                    describe_names(&offer.names, offer.items),
                    if offer.items == 1 { "it" } else { "them" }
                );
                // Replace the old clipboard at once, so a paste never brings back stale files.
                let placeholder =
                    self.write_local(&clip, ClipData { text: Some(text), ..ClipData::default() }, origin).await?;
                *lock(&self.pending) = Some(Pending { offer, conn, origin, peer, placeholder });
                self.hold.send_replace(true);
                Ok(Received::Offer)
            }
            _ => {
                // Not allowed or unknown: refuse without reading.
                let _ = stream.stop(0u32.into());
                Ok(Received::Nothing)
            }
        }
    }

    /// Writes our clipboard; returns its new sequence number.
    async fn write_local(&self, clip: &SharedClip, data: ClipData, origin: Option<DeviceId>) -> Result<u64, String> {
        let clip = clip.clone();
        let seq = tokio::task::spawn_blocking(move || {
            let mut c = lock(&clip);
            c.write(&data).map(|()| c.sequence())
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        *lock(&self.written) = Some((seq, origin));
        Ok(seq)
    }

    /// A newer clipboard arrived: forget an older file offer.
    fn drop_pending(&self) {
        if lock(&self.pending).take().is_some() {
            self.hold.send_replace(false);
        }
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

/// "report.pdf", "report.pdf and 2 more".
fn describe_names(names: &[String], items: u32) -> String {
    match names.first() {
        None => "Files".into(),
        Some(first) if items <= 1 => first.clone(),
        Some(first) => format!("{first} and {} more", items - 1),
    }
}

fn human(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let b = bytes as f64;
    match bytes {
        0..1_000_000 => format!("{:.0} KB", b / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", b / 1e6),
        _ => format!("{:.2} GB", b / 1e9),
    }
}

/// Reads and sanitises an offer (untrusted peer data).
async fn read_offer(stream: &mut quinn::RecvStream) -> Result<FileOffer, String> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).await.map_err(|e| e.to_string())?;
    let len = u32::from_le_bytes(len) as usize;
    if len > FileOffer::MAX_BYTES {
        return Err("file offer too large".into());
    }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).await.map_err(|e| e.to_string())?;
    let mut offer: FileOffer = postcard::from_bytes(&body).map_err(|_| "bad file offer".to_owned())?;
    offer.names.truncate(FileOffer::MAX_NAMES);
    for n in &mut offer.names {
        *n = n.chars().filter(|c| !c.is_control()).take(255).collect();
    }
    Ok(offer)
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
