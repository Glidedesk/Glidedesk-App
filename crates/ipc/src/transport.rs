//! Private local socket between the agent and the UI.
//!
//! * macOS/Linux: Unix socket inside the per-user config directory (`0700`),
//!   so only the owning user can connect.
//! * Windows: named pipe `\\.\pipe\nexpingdesk-agent-<user>` with a DACL that
//!   grants access only to the owner and SYSTEM, remote clients rejected.

use std::io;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// Longest accepted line (config import can be a few MB).
pub const MAX_LINE: usize = 8 * 1024 * 1024;

/// Where the agent listens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint(pub String);

impl Endpoint {
    /// Default endpoint for the current user.
    pub fn default_for_user() -> io::Result<Self> {
        #[cfg(unix)]
        {
            let store = nexpingdesk_config::ConfigStore::default_location()
                .map_err(|e| io::Error::new(io::ErrorKind::NotFound, e.to_string()))?;
            nexpingdesk_config::store::create_private_dir(store.dir())
                .map_err(|e| io::Error::new(io::ErrorKind::PermissionDenied, e.to_string()))?;
            Ok(Self(store.dir().join("agent.sock").to_string_lossy().into_owned()))
        }
        #[cfg(windows)]
        {
            let user: String = std::env::var("USERNAME")
                .unwrap_or_else(|_| "user".into())
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                .take(64)
                .collect();
            // A separate $NEXPINGDESK_HOME instance gets its own pipe.
            let suffix = std::env::var("NEXPINGDESK_HOME")
                .ok()
                .filter(|h| !h.is_empty())
                .map(|h| format!("-{:08x}", h.bytes().fold(0u32, |a, b| a.rotate_left(5) ^ u32::from(b))))
                .unwrap_or_default();
            Ok(Self(format!(r"\\.\pipe\nexpingdesk-agent-{user}{suffix}")))
        }
    }
}

/// Reads one `\n`-terminated line, refusing lines above [`MAX_LINE`].
/// Returns `Ok(None)` at end of stream.
pub async fn read_line<R: AsyncRead + Unpin>(r: &mut BufReader<R>, buf: &mut Vec<u8>) -> io::Result<Option<()>> {
    buf.clear();
    loop {
        let chunk = r.fill_buf().await?;
        if chunk.is_empty() {
            return if buf.is_empty() { Ok(None) } else { Err(io::ErrorKind::UnexpectedEof.into()) };
        }
        if let Some(pos) = chunk.iter().position(|b| *b == b'\n') {
            buf.extend_from_slice(&chunk[..pos]);
            r.consume(pos + 1);
            return Ok(Some(()));
        }
        let n = chunk.len();
        buf.extend_from_slice(chunk);
        r.consume(n);
        if buf.len() > MAX_LINE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "IPC line too long"));
        }
    }
}

pub async fn write_line<W: AsyncWrite + Unpin>(w: &mut W, json: &[u8]) -> io::Result<()> {
    w.write_all(json).await?;
    w.write_all(b"\n").await?;
    w.flush().await
}

// ---------------------------------------------------------------------------
// unix
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod imp {
    use std::io;
    use std::path::Path;

    use tokio::net::{UnixListener, UnixStream};

    use super::Endpoint;

    pub type Stream = UnixStream;

    #[derive(Debug)]
    pub struct Listener {
        inner: UnixListener,
        path: std::path::PathBuf,
    }

    impl Listener {
        /// Binds; fails with `AddrInUse` if another agent is already listening.
        pub async fn bind(ep: &Endpoint) -> io::Result<Self> {
            let path = Path::new(&ep.0);
            if path.exists() {
                if UnixStream::connect(path).await.is_ok() {
                    return Err(io::Error::new(io::ErrorKind::AddrInUse, "another agent is running"));
                }
                // Stale socket from a crashed agent.
                std::fs::remove_file(path)?;
            }
            let inner = UnixListener::bind(path)?;
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            }
            Ok(Self { inner, path: path.to_owned() })
        }

        pub async fn accept(&mut self) -> io::Result<Stream> {
            let (s, _) = self.inner.accept().await?;
            // Defence in depth: only our own user may talk to us.
            let cred = s.peer_cred()?;
            // The socket file is owned by the agent's user; peers must match it.
            let owner = {
                use std::os::unix::fs::MetadataExt as _;
                std::fs::metadata(&self.path)?.uid()
            };
            if cred.uid() != owner {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "peer is another user"));
            }
            Ok(s)
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    pub async fn connect(ep: &Endpoint) -> io::Result<Stream> {
        UnixStream::connect(&ep.0).await
    }
}

// ---------------------------------------------------------------------------
// windows
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use std::io;
    use std::time::Duration;

    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
    use windows::Win32::Foundation::{ERROR_PIPE_BUSY, HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows::core::w;

    use super::Endpoint;

    pub type Stream = NamedPipeServer;
    pub type ClientStream = tokio::net::windows::named_pipe::NamedPipeClient;

    /// Protected DACL: full access for the owner and SYSTEM only.
    struct Security {
        sd: PSECURITY_DESCRIPTOR,
    }

    impl Security {
        fn new() -> io::Result<Self> {
            let mut sd = PSECURITY_DESCRIPTOR::default();
            // SAFETY: constant SDDL string; `sd` receives a LocalAlloc'ed descriptor freed in Drop.
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    w!("D:P(A;;GA;;;OW)(A;;GA;;;SY)"),
                    SDDL_REVISION_1,
                    &raw mut sd,
                    None,
                )
            }
            .map_err(|e| io::Error::other(e.to_string()))?;
            Ok(Self { sd })
        }

        fn attributes(&self) -> SECURITY_ATTRIBUTES {
            SECURITY_ATTRIBUTES {
                nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
                lpSecurityDescriptor: self.sd.0,
                bInheritHandle: false.into(),
            }
        }
    }

    impl Drop for Security {
        fn drop(&mut self) {
            // SAFETY: descriptor allocated by ConvertStringSecurityDescriptor…
            let _ = unsafe { LocalFree(Some(HLOCAL(self.sd.0))) };
        }
    }

    // SAFETY: the descriptor is immutable after creation.
    unsafe impl Send for Security {}
    unsafe impl Sync for Security {}

    pub struct Listener {
        name: String,
        next: Option<NamedPipeServer>,
        security: Security,
    }

    impl std::fmt::Debug for Listener {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Listener").field("name", &self.name).finish_non_exhaustive()
        }
    }

    fn create(name: &str, security: &Security, first: bool) -> io::Result<NamedPipeServer> {
        let mut attrs = security.attributes();
        let mut opts = ServerOptions::new();
        opts.first_pipe_instance(first).reject_remote_clients(true);
        // SAFETY: `attrs` points to a valid SECURITY_ATTRIBUTES for the duration of the call.
        unsafe { opts.create_with_security_attributes_raw(name, (&raw mut attrs).cast()) }
    }

    impl Listener {
        pub async fn bind(ep: &Endpoint) -> io::Result<Self> {
            // Same async signature as the Unix listener (which probes the old socket).
            tokio::task::yield_now().await;
            let security = Security::new()?;
            let first = create(&ep.0, &security, true).map_err(|e| {
                if e.kind() == io::ErrorKind::PermissionDenied {
                    io::Error::new(io::ErrorKind::AddrInUse, "another agent is running")
                } else {
                    e
                }
            })?;
            Ok(Self { name: ep.0.clone(), next: Some(first), security })
        }

        pub async fn accept(&mut self) -> io::Result<Stream> {
            let server = match self.next.take() {
                Some(s) => s,
                None => create(&self.name, &self.security, false)?,
            };
            server.connect().await?;
            self.next = Some(create(&self.name, &self.security, false)?);
            Ok(server)
        }
    }

    pub async fn connect(ep: &Endpoint) -> io::Result<ClientStream> {
        for _ in 0..50 {
            match ClientOptions::new().open(&ep.0) {
                Ok(c) => return Ok(c),
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY.0.cast_signed()) => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(io::ErrorKind::TimedOut, "agent pipe busy"))
    }
}

pub use imp::{Listener, Stream, connect};
