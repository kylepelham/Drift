use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::service::TowerToHyperService;
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, SanType};
use rustls::pki_types::PrivatePkcs8KeyDer;

use super::*;

/// A certificate authority: a PC's own, as the desktop's gateway makes one, or a stand-in for a public one.
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
        self.serve_leaf(router).await.0
    }

    /// As `serve`, also handing back the leaf, whose key a tunnel pin names.
    pub(crate) async fn serve_leaf(&self, router: Router) -> (SocketAddr, CertificateDer<'static>) {
        let mut params = CertificateParams::default();
        params.subject_alt_names = vec![SanType::IpAddress("127.0.0.1".parse().unwrap())];
        let key = KeyPair::generate().unwrap();
        let leaf = params.signed_by(&key, &self.issuer).unwrap().der().clone();

        let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![leaf.clone(), self.der.clone()],
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

        (address, leaf)
    }

    /// A root store holding only this authority, standing in for the public roots.
    pub(crate) fn as_public_roots(&self) -> Arc<RootCertStore> {
        let mut roots = RootCertStore::empty();
        roots.add(self.der.clone()).unwrap();
        Arc::new(roots)
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

    let pinned = client(Trust::Authority(pc.fingerprint()));
    assert_eq!(get(&pinned, address).await.unwrap(), "hello");

    let impostor = client(Trust::Authority(Authority::new().fingerprint()));
    assert!(get(&impostor, address).await.is_err(), "another authority is refused");
}

#[tokio::test]
async fn through_the_tunnel_only_a_publicly_trusted_certificate_with_the_pcs_key_is_trusted() {
    let public = Authority::new();
    let (address, leaf) = public.serve_leaf(hello()).await;
    let key = key_of(&leaf).unwrap();

    let pinned = build(Trust::Key(key), public.as_public_roots());
    assert_eq!(get(&pinned, address).await.unwrap(), "hello");

    let relay = build(Trust::Key([7; 32]), public.as_public_roots());
    assert!(
        get(&relay, address).await.is_err(),
        "a valid certificate for the name with another key is refused"
    );

    let unknown = build(Trust::Key(key), Arc::new(RootCertStore::empty()));
    assert!(
        get(&unknown, address).await.is_err(),
        "the right key under an untrusted issuer is refused"
    );
}
