use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::service::TowerToHyperService;
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, SanType};
use rustls::pki_types::PrivatePkcs8KeyDer;

use super::*;

/// A PC's certificate authority, as the desktop's gateway makes one.
pub(crate) struct Authority {
    issuer: Issuer<'static, KeyPair>,
    pub der: CertificateDer<'static>,
}

impl Authority {
    pub(crate) fn new() -> Self {
        let mut params = CertificateParams::default();
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        let key = KeyPair::generate().unwrap();
        let der = params.self_signed(&key).unwrap().der().clone();

        Self {
            issuer: Issuer::new(params, key),
            der,
        }
    }

    pub(crate) fn fingerprint(&self) -> Fingerprint {
        fingerprint(&self.der)
    }

    /// Serves `router` over HTTPS on 127.0.0.1 with a leaf this authority signed for that address.
    pub(crate) async fn serve(&self, router: Router) -> SocketAddr {
        let mut params = CertificateParams::default();
        params.subject_alt_names = vec![SanType::IpAddress("127.0.0.1".parse().unwrap())];
        let key = KeyPair::generate().unwrap();
        let leaf = params.signed_by(&key, &self.issuer).unwrap();

        let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![leaf.der().clone(), self.der.clone()],
                PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
            )
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();

        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (acceptor, router) = (acceptor.clone(), router.clone());
                tokio::spawn(async move {
                    let Ok(stream) = acceptor.accept(stream).await else {
                        return;
                    };
                    let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                        .serve_connection_with_upgrades(TokioIo::new(stream), TowerToHyperService::new(router))
                        .await;
                });
            }
        });

        address
    }
}

fn hello() -> Router {
    Router::new().route("/hello", axum::routing::get(|| async { "hello" }))
}

async fn get(client: &reqwest::Client, address: SocketAddr) -> Result<String, reqwest::Error> {
    client
        .get(format!("https://{address}/hello"))
        .send()
        .await?
        .text()
        .await
}

#[tokio::test]
async fn only_the_pinned_authoritys_pc_is_trusted() {
    let pc = Authority::new();
    let address = pc.serve(hello()).await;

    let (pinned, _) = client(Some(pc.fingerprint()));
    assert_eq!(get(&pinned, address).await.unwrap(), "hello");

    let (impostor, _) = client(Some(Authority::new().fingerprint()));
    assert!(get(&impostor, address).await.is_err(), "another authority is refused");
}

#[tokio::test]
async fn a_pc_met_by_address_is_trusted_once_and_held_to_that_authority() {
    let pc = Authority::new();
    let address = pc.serve(hello()).await;
    let (first, trusted) = client(None);

    assert_eq!(get(&first, address).await.unwrap(), "hello");
    assert_eq!(
        trusted.get(),
        Some(pc.fingerprint()),
        "its words come from the authority it met"
    );

    // The same client meeting a different PC later refuses it.
    let other = Authority::new();
    let moved = other.serve(hello()).await;
    assert!(get(&first, moved).await.is_err());
}
