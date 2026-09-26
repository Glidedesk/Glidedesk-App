//! Optional server password.
//!
//! * The server stores only `key = Argon2id(password, salt)` and the salt,
//!   never the password.
//! * Both sides run SPAKE2 (Ed25519) on `key`. The password never crosses the
//!   network, and a recorded exchange can't be brute-forced offline.
//! * Each side proves the shared secret with an HMAC over the TLS exporter of
//!   *this* QUIC connection. A man in the middle terminates two different TLS
//!   sessions, so it can't relay a proof. The server proves too (mutual).

use hmac::{Hmac, Mac};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};

pub const SALT_LEN: usize = 16;
pub const KEY_LEN: usize = 32;
pub const PROOF_LEN: usize = 32;
/// Label for `Connection::export_keying_material`.
pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-glidedesk-auth-v1";
/// Longest SPAKE2 message we accept (Ed25519: 33 bytes).
pub const MAX_MSG: usize = 64;

const ID_CLIENT: &[u8] = b"glidedesk client";
const ID_SERVER: &[u8] = b"glidedesk server";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("the password is wrong")]
    WrongPassword,
    #[error("malformed authentication message")]
    Malformed,
    #[error("password hashing failed: {0}")]
    Kdf(String),
}

/// What the server keeps in its settings: salt and derived key (hex in TOML).
#[derive(Clone, PartialEq, Eq)]
pub struct Verifier {
    pub salt: [u8; SALT_LEN],
    pub key: [u8; KEY_LEN],
}

impl std::fmt::Debug for Verifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Verifier(..)")
    }
}

/// Argon2id with the OWASP minimum (19 MiB, 2 passes): ~50 ms on a laptop.
pub fn derive_key(password: &str, salt: &[u8; SALT_LEN]) -> Result<[u8; KEY_LEN], AuthError> {
    use argon2::{Algorithm, Argon2, Params, Version};
    let params = Params::new(19 * 1024, 2, 1, Some(KEY_LEN)).map_err(|e| AuthError::Kdf(e.to_string()))?;
    let mut out = [0u8; KEY_LEN];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut out)
        .map_err(|e| AuthError::Kdf(e.to_string()))?;
    Ok(out)
}

impl Verifier {
    /// New verifier with a random salt.
    pub fn new(password: &str) -> Result<Self, AuthError> {
        let mut salt = [0u8; SALT_LEN];
        getrandom::fill(&mut salt).map_err(|e| AuthError::Kdf(e.to_string()))?;
        Ok(Self { salt, key: derive_key(password, &salt)? })
    }

    #[must_use]
    pub fn to_hex(&self) -> (String, String) {
        (hex(&self.salt), hex(&self.key))
    }

    #[must_use]
    pub fn from_hex(salt: &str, key: &str) -> Option<Self> {
        Some(Self { salt: unhex(salt)?, key: unhex(key)? })
    }
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter().fold(String::with_capacity(b.len() * 2), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let s = s.trim();
    if s.len() != N * 2 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

fn proof(secret: &[u8], role: &[u8], exporter: &[u8]) -> Result<[u8; PROOF_LEN], AuthError> {
    // HMAC accepts keys of any length, so this never fails in practice.
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret).map_err(|_| AuthError::Malformed)?;
    mac.update(role);
    mac.update(exporter);
    Ok(mac.finalize().into_bytes().into())
}

fn verify(secret: &[u8], role: &[u8], exporter: &[u8], got: &[u8]) -> bool {
    let Ok(mut mac) = <Hmac<Sha256> as Mac>::new_from_slice(secret) else { return false };
    mac.update(role);
    mac.update(exporter);
    mac.verify_slice(got).is_ok() // constant time
}

/// Server side of one handshake.
pub struct ServerChallenge {
    state: Spake2<Ed25519Group>,
    /// Sent to the client with the salt.
    pub message: Vec<u8>,
}

impl std::fmt::Debug for ServerChallenge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServerChallenge(..)")
    }
}

impl ServerChallenge {
    #[must_use]
    pub fn start(v: &Verifier) -> Self {
        let (state, message) = Spake2::<Ed25519Group>::start_b(
            &Password::new(v.key),
            &Identity::new(ID_CLIENT),
            &Identity::new(ID_SERVER),
        );
        Self { state, message }
    }

    /// Checks the client's answer; returns the server's own proof.
    pub fn finish(
        self,
        client_message: &[u8],
        client_proof: &[u8],
        exporter: &[u8],
    ) -> Result<[u8; PROOF_LEN], AuthError> {
        if client_message.len() > MAX_MSG || client_proof.len() != PROOF_LEN {
            return Err(AuthError::Malformed);
        }
        let secret = self.state.finish(client_message).map_err(|_| AuthError::WrongPassword)?;
        if !verify(&secret, b"client", exporter, client_proof) {
            return Err(AuthError::WrongPassword);
        }
        proof(&secret, b"server", exporter)
    }
}

/// The client's answer to a challenge.
#[derive(Debug)]
pub struct ClientAnswer {
    pub message: Vec<u8>,
    pub proof: [u8; PROOF_LEN],
    /// What the server must send back to prove it knows the password too.
    pub expected_server_proof: [u8; PROOF_LEN],
}

/// Client side: derive the key from the password and answer the challenge.
pub fn client_answer(
    password: &str,
    salt: &[u8],
    server_message: &[u8],
    exporter: &[u8],
) -> Result<ClientAnswer, AuthError> {
    let salt: [u8; SALT_LEN] = salt.try_into().map_err(|_| AuthError::Malformed)?;
    if server_message.len() > MAX_MSG {
        return Err(AuthError::Malformed);
    }
    let key = derive_key(password, &salt)?;
    let (state, message) =
        Spake2::<Ed25519Group>::start_a(&Password::new(key), &Identity::new(ID_CLIENT), &Identity::new(ID_SERVER));
    let secret = state.finish(server_message).map_err(|_| AuthError::Malformed)?;
    Ok(ClientAnswer {
        message,
        proof: proof(&secret, b"client", exporter)?,
        expected_server_proof: proof(&secret, b"server", exporter)?,
    })
}

/// Constant-time check of the server's proof.
#[must_use]
pub fn server_proof_ok(expected: &[u8; PROOF_LEN], got: &[u8]) -> bool {
    got.len() == PROOF_LEN && expected.iter().zip(got).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// Failed-password throttle per peer IP: 5 free tries, then
/// 60 s blocked, doubling up to 1 h. Bounded memory.
#[derive(Debug, Default)]
pub struct Throttle {
    peers: std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, (u32, std::time::Instant)>>,
    /// Failures from all peers in the current minute (window start, count,
    /// blocked until): a guesser rotating addresses still hits this.
    global: std::sync::Mutex<Option<(std::time::Instant, u32, std::time::Instant)>>,
}

const FREE_TRIES: u32 = 5;
const MAX_PEERS: usize = 4096;
/// Wrong passwords per minute from everyone together before all wait a minute.
const GLOBAL_PER_MINUTE: u32 = 30;

/// One bucket per IPv4 address or IPv6 /64: a single LAN host can use any
/// number of addresses inside its /64.
fn bucket(ip: std::net::IpAddr) -> std::net::IpAddr {
    match ip {
        std::net::IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                std::net::IpAddr::V4(v4)
            } else {
                let s = v6.segments();
                std::net::IpAddr::V6(std::net::Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
        }
        v4 @ std::net::IpAddr::V4(_) => v4,
    }
}

impl Throttle {
    fn lock(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<std::net::IpAddr, (u32, std::time::Instant)>> {
        self.peers.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Is this peer currently locked out?
    #[must_use]
    pub fn blocked(&self, ip: std::net::IpAddr, now: std::time::Instant) -> bool {
        let global_block =
            self.global.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some_and(|(_, _, u)| u > now);
        global_block || self.lock().get(&bucket(ip)).is_some_and(|(_, until)| *until > now)
    }

    fn fail_global(&self, now: std::time::Instant) {
        let mut g = self.global.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (start, count, until) = match *g {
            Some((start, count, until)) if now.duration_since(start) < std::time::Duration::from_secs(60) => {
                (start, count.saturating_add(1), until)
            }
            Some((_, _, until)) => (now, 1, until),
            None => (now, 1, now),
        };
        let until = if count >= GLOBAL_PER_MINUTE { now + std::time::Duration::from_secs(60) } else { until };
        *g = Some((start, count, until));
    }

    /// Records a wrong password; returns how long the peer is now blocked.
    pub fn fail(&self, ip: std::net::IpAddr, now: std::time::Instant) -> std::time::Duration {
        self.fail_global(now);
        let ip = bucket(ip);
        let mut peers = self.lock();
        if peers.len() >= MAX_PEERS && !peers.contains_key(&ip) {
            peers.retain(|_, (_, until)| *until > now);
            if peers.len() >= MAX_PEERS {
                return std::time::Duration::ZERO;
            }
        }
        let e = peers.entry(ip).or_insert((0, now));
        e.0 = e.0.saturating_add(1);
        let wait = if e.0 < FREE_TRIES {
            std::time::Duration::ZERO
        } else {
            let doublings = (e.0 - FREE_TRIES).min(6);
            std::time::Duration::from_secs((60u64 << doublings).min(3600))
        };
        e.1 = now + wait;
        wait
    }

    pub fn success(&self, ip: std::net::IpAddr) {
        self.lock().remove(&bucket(ip));
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    const EXP: &[u8] = b"tls-exporter-of-this-connection";

    #[test]
    fn right_password_authenticates_both_ways() {
        let v = Verifier::new("correct horse").unwrap();
        let ch = ServerChallenge::start(&v);
        let ans = client_answer("correct horse", &v.salt, &ch.message, EXP).unwrap();
        let server_proof = ch.finish(&ans.message, &ans.proof, EXP).unwrap();
        assert!(server_proof_ok(&ans.expected_server_proof, &server_proof));
    }

    #[test]
    fn wrong_password_is_refused() {
        let v = Verifier::new("correct horse").unwrap();
        let ch = ServerChallenge::start(&v);
        let ans = client_answer("battery staple", &v.salt, &ch.message, EXP).unwrap();
        assert_eq!(ch.finish(&ans.message, &ans.proof, EXP), Err(AuthError::WrongPassword));
    }

    #[test]
    fn proofs_are_bound_to_the_tls_session() {
        // A relay (man in the middle) sees a different exporter on each side.
        let v = Verifier::new("pw").unwrap();
        let ch = ServerChallenge::start(&v);
        let ans = client_answer("pw", &v.salt, &ch.message, b"client-side session").unwrap();
        assert_eq!(ch.finish(&ans.message, &ans.proof, b"server-side session"), Err(AuthError::WrongPassword));
    }

    #[test]
    fn garbage_never_panics() {
        let v = Verifier::new("pw").unwrap();
        for msg in [&[][..], &[0u8; 33][..], &[0xff; 64][..], &[1u8; 200][..]] {
            let ch = ServerChallenge::start(&v);
            assert!(ch.finish(msg, &[0u8; PROOF_LEN], EXP).is_err());
            let _ = client_answer("pw", &v.salt, msg, EXP); // must not panic
        }
        assert!(client_answer("pw", &[1, 2, 3], &[0u8; 33], EXP).is_err());
    }

    #[test]
    fn verifier_hex_roundtrip_and_no_password_in_debug() {
        let v = Verifier::new("secret").unwrap();
        let (s, k) = v.to_hex();
        assert_eq!(Verifier::from_hex(&s, &k), Some(v.clone()));
        assert!(Verifier::from_hex("zz", &k).is_none());
        assert_eq!(format!("{v:?}"), "Verifier(..)");
        assert_ne!(Verifier::new("secret").unwrap().key, v.key, "salt is random");
    }

    #[test]
    fn throttle_blocks_after_five_failures_and_doubles() {
        let t = Throttle::default();
        let ip: std::net::IpAddr = "192.168.1.9".parse().unwrap();
        let now = Instant::now();
        for _ in 0..4 {
            assert_eq!(t.fail(ip, now), Duration::ZERO);
        }
        assert!(!t.blocked(ip, now));
        assert_eq!(t.fail(ip, now), Duration::from_secs(60));
        assert!(t.blocked(ip, now));
        assert_eq!(t.fail(ip, now), Duration::from_secs(120));
        assert!(!t.blocked(ip, now + Duration::from_secs(121)));
        t.success(ip);
        assert!(!t.blocked(ip, now));
    }

    #[test]
    fn ipv6_addresses_in_one_64_share_a_bucket_and_a_global_cap_applies() {
        let t = Throttle::default();
        let now = Instant::now();
        for i in 0..5u16 {
            let ip = std::net::IpAddr::V6(std::net::Ipv6Addr::new(0xfd00, 1, 2, 3, i, 7, 7, i));
            let _ = t.fail(ip, now);
        }
        assert!(t.blocked("fd00:1:2:3:dead:beef::1".parse().unwrap(), now), "rotating inside a /64 doesn't help");
        assert!(!t.blocked("fd00:1:2:4::1".parse().unwrap(), now), "another /64 is another host");

        let g = Throttle::default();
        for i in 0..30u8 {
            let _ = g.fail(std::net::IpAddr::V4(std::net::Ipv4Addr::new(10, 0, i, 1)), now);
        }
        assert!(g.blocked("192.168.9.9".parse().unwrap(), now), "global cap");
        assert!(!g.blocked("192.168.9.9".parse().unwrap(), now + Duration::from_secs(61)));
    }
}
