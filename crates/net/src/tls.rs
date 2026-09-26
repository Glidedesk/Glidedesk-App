//! TLS for QUIC. There is no authentication by design (see SECURITY.md): the server
//! uses a fresh self-signed certificate and clients accept any certificate,
//! but the TLS 1.3 handshake signatures are still verified, so traffic is
//! encrypted against passive listeners on the LAN.

use std::sync::Arc;

use glidedesk_proto::ALPN;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

use crate::NetError;

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Server TLS config with a newly generated certificate.
pub fn server_config() -> Result<(rustls::ServerConfig, [u8; 32]), NetError> {
    let ck =
        rcgen::generate_simple_self_signed(vec!["glidedesk".to_owned()]).map_err(|e| NetError::Tls(e.to_string()))?;
    let cert = ck.cert.der().clone();
    let fingerprint = fingerprint(&cert);
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der()));
    let mut cfg = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| NetError::Tls(e.to_string()))?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .map_err(|e| NetError::Tls(e.to_string()))?;
    cfg.alpn_protocols = vec![ALPN.to_vec()];
    Ok((cfg, fingerprint))
}

/// Client TLS config that accepts any server certificate (see module docs).
pub fn client_config() -> Result<rustls::ClientConfig, NetError> {
    let p = provider();
    let verifier = Arc::new(AnyServerCert { algorithms: p.signature_verification_algorithms });
    let mut cfg = rustls::ClientConfig::builder_with_provider(p)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| NetError::Tls(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    cfg.alpn_protocols = vec![ALPN.to_vec()];
    Ok(cfg)
}

/// Stable short fingerprint of a certificate (shown in diagnostics).
#[must_use]
pub fn fingerprint(cert: &CertificateDer<'_>) -> [u8; 32] {
    *blake3::hash(cert.as_ref()).as_bytes()
}

#[derive(Debug)]
struct AnyServerCert {
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for AnyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        // QUIC is TLS 1.3 only.
        Err(rustls::Error::PeerIncompatible(rustls::PeerIncompatible::Tls12NotOffered))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}
