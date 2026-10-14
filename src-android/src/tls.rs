//! Connections that trust exactly one PC: its Remote Access certificate authority, by SHA-256.
//! A PC first met by address (linking by typed code) is trusted on that first contact and held to it after.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error, RootCertStore, SignatureScheme};
use sha2::{Digest, Sha256};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) type Fingerprint = [u8; 32];

/// The authority a connection has trusted, shared with whoever needs to know (the six words while linking).
#[derive(Clone, Debug, Default)]
pub(crate) struct Trusted(Arc<Mutex<Option<Fingerprint>>>);

impl Trusted {
    pub(crate) fn get(&self) -> Option<Fingerprint> {
        *self.0.lock().unwrap()
    }
}

#[derive(Debug)]
struct Pinned {
    trusted: Trusted,
    provider: Arc<CryptoProvider>,
}

/// A client for one PC. With `fingerprint`, only that authority is trusted; without, the first one seen is.
pub(crate) fn client(fingerprint: Option<Fingerprint>) -> (reqwest::Client, Trusted) {
    let trusted = Trusted(Arc::new(Mutex::new(fingerprint)));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Pinned {
        trusted: trusted.clone(),
        provider: provider.clone(),
    };

    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    // HTTP/1.1 only, so the event socket's upgrade passes through as the WebView asked for it.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];

    let client = reqwest::Client::builder()
        .tls_backend_preconfigured(config)
        .http1_only()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .expect("a client with a preconfigured TLS backend builds");

    (client, trusted)
}

pub(crate) fn fingerprint(certificate: &[u8]) -> Fingerprint {
    Sha256::digest(certificate).into()
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        leaf: &CertificateDer<'_>,
        chain: &[CertificateDer<'_>],
        name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let pinned = self.trusted.get();
        let authority = match pinned {
            Some(pinned) => chain.iter().find(|certificate| fingerprint(certificate) == pinned),
            // The gateway sends its authority last in the chain.
            None => chain.last(),
        }
        .ok_or_else(|| Error::General("this is not the PC Drift linked with".into()))?;

        // The leaf must be issued by that authority, for the address connected to, as any TLS client checks.
        let mut roots = RootCertStore::empty();
        roots.add(authority.clone())?;
        let webpki = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), self.provider.clone())
            .build()
            .map_err(|error| Error::General(error.to_string()))?;
        webpki.verify_server_cert(leaf, &[], name, ocsp, now)?;

        if pinned.is_none() {
            *self.trusted.0.lock().unwrap() = Some(fingerprint(authority));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(
            message,
            certificate,
            signature,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(
            message,
            certificate,
            signature,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
pub(crate) mod tests;
