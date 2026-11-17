//! Connections that trust exactly one PC. On a network that is its Remote Access certificate authority, by SHA-256.
//! Through OpenTunnel it is a publicly trusted certificate for the tunnel's name that also carries the PC's own key,
//! which never leaves the PC, so not even the relay that controls the name can stand in for it. A PC not linked yet
//! is trusted on first use: whatever authority it shows is the one every later connection must show, and the code
//! the two screens compare is what proves it was the PC (`drift_pairing`).

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error, RootCertStore, SignatureScheme};
use sha2::{Digest, Sha256};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

pub type Fingerprint = [u8; 32];

/// What a connection to a PC trusts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Trust {
    /// The PC's own certificate authority, by fingerprint.
    Authority(Fingerprint),
    /// A publicly trusted certificate whose public key has this SHA-256.
    Key(Fingerprint),
}

#[derive(Debug)]
struct Pinned {
    /// `None` on first use, when the authority met first is pinned in `met`.
    trust: Option<Trust>,
    met: Arc<OnceLock<Fingerprint>>,
    public: Arc<RootCertStore>,
    provider: Arc<CryptoProvider>,
}

/// A client for one PC that trusts only what `trust` names.
pub fn client(trust: Trust) -> reqwest::Client {
    build(Some(trust), Arc::default(), public_roots())
}

/// A client for a PC not linked yet, and the authority it met, which every later connection of it must meet again.
pub fn first_use() -> (reqwest::Client, Arc<OnceLock<Fingerprint>>) {
    let met = Arc::new(OnceLock::new());
    (build(None, met.clone(), public_roots()), met)
}

fn public_roots() -> Arc<RootCertStore> {
    static ROOTS: OnceLock<Arc<RootCertStore>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let mut roots = RootCertStore::empty();
            roots.add_parsable_certificates(webpki_root_certs::TLS_SERVER_ROOT_CERTS.iter().cloned());
            Arc::new(roots)
        })
        .clone()
}

fn build(trust: Option<Trust>, met: Arc<OnceLock<Fingerprint>>, public: Arc<RootCertStore>) -> reqwest::Client {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Pinned {
        trust,
        met,
        public,
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

    reqwest::Client::builder()
        .tls_backend_preconfigured(config)
        .http1_only()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .expect("a client with a preconfigured TLS backend builds")
}

pub fn fingerprint(certificate: &[u8]) -> Fingerprint {
    Sha256::digest(certificate).into()
}

/// SHA-256 of a certificate's public key, as the PC reports it for its tunnel.
pub fn key_of(certificate: &CertificateDer<'_>) -> Option<Fingerprint> {
    let certificate = webpki::EndEntityCert::try_from(certificate).ok()?;
    Some(Sha256::digest(certificate.subject_public_key_info().as_ref()).into())
}

impl Pinned {
    fn verifier(&self, roots: Arc<RootCertStore>) -> Result<Arc<WebPkiServerVerifier>, Error> {
        WebPkiServerVerifier::builder_with_provider(roots, self.provider.clone())
            .build()
            .map_err(|error| Error::General(error.to_string()))
    }
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
        let not_it = || Error::General("this is not the PC Drift linked with".into());
        // Whether an authority issued the leaf for this name.
        let issued = |authority: &CertificateDer<'_>| -> Result<(), Error> {
            let mut roots = RootCertStore::empty();
            roots.add(authority.clone().into_owned())?;
            self.verifier(Arc::new(roots))?
                .verify_server_cert(leaf, &[], name, ocsp, now)
                .map(|_| ())
        };
        match self.trust {
            // The gateway sends its authority in the chain; the leaf must be issued by it for this address.
            Some(Trust::Authority(pinned)) => {
                let authority = chain
                    .iter()
                    .find(|certificate| fingerprint(certificate) == pinned)
                    .ok_or_else(not_it)?;
                issued(authority)?;
            }
            None => {
                let authority = chain
                    .iter()
                    .find(|authority| issued(authority).is_ok())
                    .ok_or_else(|| Error::General("this is not a Drift computer".into()))?;
                let met = fingerprint(authority);
                if *self.met.get_or_init(|| met) != met {
                    return Err(not_it());
                }
            }
            Some(Trust::Key(pinned)) => {
                self.verifier(self.public.clone())?
                    .verify_server_cert(leaf, chain, name, ocsp, now)?;
                if key_of(leaf) != Some(pinned) {
                    return Err(not_it());
                }
            }
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
