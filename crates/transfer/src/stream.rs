//! Streaming a file set over any async byte stream (a QUIC stream in the app).
//!
//! Wire format: `u32 LE length + postcard Manifest`, then for every file
//! entry in order its bytes followed by its 32-byte BLAKE3 hash.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::manifest::{Manifest, safe_join};
use crate::{TransferError, io_err};

const CHUNK: usize = 256 * 1024;
/// A manifest with 500k entries is a few tens of MB at most.
const MAX_MANIFEST: usize = 64 * 1024 * 1024;

/// Shared progress counters (read by the UI).
#[derive(Debug, Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
}

fn net(e: impl std::fmt::Display) -> TransferError {
    TransferError::Net(e.to_string())
}

/// Sends the manifest and every file under `sources` (the copied items).
pub async fn send_set<W: AsyncWrite + Unpin>(
    w: &mut W,
    sources: &[PathBuf],
    manifest: &Manifest,
    progress: &Progress,
) -> Result<(), TransferError> {
    let body = postcard::to_stdvec(manifest).map_err(net)?;
    let len = u32::try_from(body.len()).map_err(|_| TransferError::BadManifest("manifest too large"))?;
    w.write_all(&len.to_le_bytes()).await.map_err(net)?;
    w.write_all(&body).await.map_err(net)?;
    progress.total.store(manifest.total_bytes, Ordering::Relaxed);
    let mut buf = vec![0u8; CHUNK];
    for e in manifest.entries.iter().filter(|e| !e.is_dir) {
        // Map the manifest path back to the source: first component = item name.
        let root = sources
            .iter()
            .find(|s| s.file_name().is_some_and(|n| n.to_string_lossy() == e.path[0]))
            .ok_or(TransferError::Changed(e.path.join("/")))?;
        let mut path = root.clone();
        for c in &e.path[1..] {
            path.push(c);
        }
        let mut f = tokio::fs::File::open(&path).await.map_err(io_err(&path))?;
        if f.metadata().await.map_err(io_err(&path))?.len() != e.size {
            return Err(TransferError::Changed(path.display().to_string()));
        }
        let mut hasher = blake3::Hasher::new();
        let mut left = e.size;
        while left > 0 {
            if progress.cancel.load(Ordering::Relaxed) {
                return Err(TransferError::Cancelled);
            }
            let want = usize::try_from(left.min(CHUNK as u64)).unwrap_or(CHUNK);
            let n = f.read(&mut buf[..want]).await.map_err(io_err(&path))?;
            if n == 0 {
                return Err(TransferError::Changed(path.display().to_string()));
            }
            hasher.update(&buf[..n]);
            w.write_all(&buf[..n]).await.map_err(net)?;
            left -= n as u64;
            progress.done.fetch_add(n as u64, Ordering::Relaxed);
        }
        w.write_all(hasher.finalize().as_bytes()).await.map_err(net)?;
    }
    w.flush().await.map_err(net)?;
    Ok(())
}

/// Receives a set into `dest` (an empty private folder). On any error the
/// folder's contents are removed by the caller (see `Staging::discard`).
pub async fn receive_set<R: AsyncRead + Unpin>(
    r: &mut R,
    dest: &Path,
    max_total: u64,
    progress: Arc<Progress>,
) -> Result<Manifest, TransferError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await.map_err(net)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_MANIFEST {
        return Err(TransferError::BadManifest("manifest too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await.map_err(net)?;
    let manifest: Manifest = postcard::from_bytes(&body).map_err(|_| TransferError::BadManifest("undecodable"))?;
    manifest.validate()?;
    if max_total != 0 && manifest.total_bytes > max_total {
        return Err(TransferError::BadManifest("larger than the allowed size"));
    }
    progress.total.store(manifest.total_bytes, Ordering::Relaxed);
    let mut buf = vec![0u8; CHUNK];
    for e in &manifest.entries {
        let path = safe_join(dest, &e.path);
        if e.is_dir {
            tokio::fs::create_dir_all(&path).await.map_err(io_err(&path))?;
            continue;
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(io_err(parent))?;
        }
        let mut f =
            tokio::fs::OpenOptions::new().write(true).create_new(true).open(&path).await.map_err(io_err(&path))?;
        let mut hasher = blake3::Hasher::new();
        let mut left = e.size;
        while left > 0 {
            if progress.cancel.load(Ordering::Relaxed) {
                return Err(TransferError::Cancelled);
            }
            let want = usize::try_from(left.min(CHUNK as u64)).unwrap_or(CHUNK);
            let n = r.read(&mut buf[..want]).await.map_err(net)?;
            if n == 0 {
                return Err(TransferError::Net("stream ended early".into()));
            }
            hasher.update(&buf[..n]);
            f.write_all(&buf[..n]).await.map_err(io_err(&path))?;
            left -= n as u64;
            progress.done.fetch_add(n as u64, Ordering::Relaxed);
        }
        f.flush().await.map_err(io_err(&path))?;
        let mut expect = [0u8; 32];
        r.read_exact(&mut expect).await.map_err(net)?;
        if hasher.finalize().as_bytes() != &expect {
            return Err(TransferError::Corrupt(e.path.join("/")));
        }
        let f = f.into_std().await;
        if e.mtime > 0 {
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(e.mtime.unsigned_abs());
            let _ = f.set_modified(t);
        }
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_manifest;

    #[tokio::test]
    async fn roundtrip_tree_with_verification() {
        let src = tempfile::tempdir().unwrap();
        let dir = src.path().join("proj");
        std::fs::create_dir_all(dir.join("deep/er")).unwrap();
        std::fs::write(dir.join("deep/er/big.bin"), vec![42u8; 3 * CHUNK + 17]).unwrap();
        std::fs::write(dir.join("readme.md"), b"# hi").unwrap();
        std::fs::create_dir_all(dir.join("empty")).unwrap();
        let items = vec![dir];
        let m = build_manifest(&items, false).unwrap();

        let dst = tempfile::tempdir().unwrap();
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        let sp = Progress::default();
        let rp = Arc::new(Progress::default());
        let dpath = dst.path().to_path_buf();
        let recv = tokio::spawn({
            let rp = rp.clone();
            async move { receive_set(&mut b, &dpath, 0, rp).await }
        });
        send_set(&mut a, &items, &m, &sp).await.unwrap();
        drop(a);
        let got = recv.await.unwrap().unwrap();
        assert_eq!(got, m);
        assert_eq!(std::fs::read(dst.path().join("proj/deep/er/big.bin")).unwrap().len(), 3 * CHUNK + 17);
        assert!(dst.path().join("proj/empty").is_dir());
        assert_eq!(rp.done.load(Ordering::Relaxed), m.total_bytes);
    }

    #[tokio::test]
    async fn corruption_is_detected() {
        let src = tempfile::tempdir().unwrap();
        let f = src.path().join("f.txt");
        std::fs::write(&f, b"important").unwrap();
        let m = build_manifest(std::slice::from_ref(&f), false).unwrap();
        let mut wire = Vec::new();
        send_set(&mut wire, &[f], &m, &Progress::default()).await.unwrap();
        // Flip one payload byte (right after the manifest).
        let payload_at = 4 + u32::from_le_bytes(wire[..4].try_into().unwrap()) as usize;
        wire[payload_at] ^= 0xFF;
        let dst = tempfile::tempdir().unwrap();
        let err = receive_set(&mut wire.as_slice(), dst.path(), 0, Arc::default()).await.unwrap_err();
        assert!(matches!(err, TransferError::Corrupt(_)), "{err}");
    }

    #[tokio::test]
    async fn hostile_manifest_cannot_escape() {
        let m = Manifest {
            entries: vec![crate::Entry {
                path: vec!["..".into(), "..".into(), "evil".into()],
                size: 1,
                mtime: 0,
                is_dir: false,
            }],
            total_bytes: 1,
            cut: false,
        };
        let body = postcard::to_stdvec(&m).unwrap();
        let mut wire = (u32::try_from(body.len()).unwrap()).to_le_bytes().to_vec();
        wire.extend_from_slice(&body);
        wire.push(b'x');
        wire.extend_from_slice(blake3::hash(b"x").as_bytes());
        let dst = tempfile::tempdir().unwrap();
        receive_set(&mut wire.as_slice(), dst.path(), 0, Arc::default()).await.unwrap();
        assert!(dst.path().join("_/_/evil").exists(), "written inside the staging folder");
    }

    #[tokio::test]
    async fn size_limit_and_cancel() {
        let src = tempfile::tempdir().unwrap();
        let f = src.path().join("f");
        std::fs::write(&f, vec![1u8; 100]).unwrap();
        let m = build_manifest(std::slice::from_ref(&f), false).unwrap();
        let mut wire = Vec::new();
        send_set(&mut wire, std::slice::from_ref(&f), &m, &Progress::default()).await.unwrap();
        let dst = tempfile::tempdir().unwrap();
        assert!(receive_set(&mut wire.as_slice(), dst.path(), 50, Arc::default()).await.is_err());
        let p = Progress::default();
        p.cancel.store(true, Ordering::Relaxed);
        let mut sink = Vec::new();
        assert!(matches!(send_set(&mut sink, &[f], &m, &p).await, Err(TransferError::Cancelled)));
    }
}
