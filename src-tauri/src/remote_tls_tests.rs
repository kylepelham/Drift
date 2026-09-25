use super::*;
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::{ServerName, UnixTime};
use std::net::Ipv4Addr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn directory() -> std::path::PathBuf {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).unwrap();
    std::env::temp_dir().join(format!("drift-remote-tls-{}", u64::from_ne_bytes(bytes)))
}

fn roots(tls: &Tls) -> Arc<rustls::RootCertStore> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(tls.ca_der().to_vec())).unwrap();
    Arc::new(roots)
}

fn verify(tls: &Tls, address: IpAddr) -> Result<(), rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(roots(tls), provider).build().unwrap();
    let (chain, _) = tls.leaf(address).unwrap();
    let name = ServerName::IpAddress(address.into());
    verifier.verify_server_cert(&chain[0], &chain[1..], &name, &[], UnixTime::now()).map(|_| ())
}

#[test]
fn the_authority_is_created_once_and_reloaded() {
    let root = directory();
    let first = Tls::load_or_create(&root).unwrap();
    let again = Tls::load_or_create(&root).unwrap();
    assert_eq!(first.fingerprint(), again.fingerprint());
    assert_eq!(first.fingerprint().split(':').count(), 32);
    std::fs::write(root.join(CA_KEY_FILE), "corrupt").unwrap();
    let replaced = Tls::load_or_create(&root).unwrap();
    assert_ne!(replaced.fingerprint(), first.fingerprint(), "an unreadable key pair is regenerated");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn leaves_verify_for_private_addresses_even_after_reload() {
    let root = directory();
    let created = Tls::load_or_create(&root).unwrap();
    let reloaded = Tls::load_or_create(&root).unwrap();
    for address in [[192, 168, 1, 20], [10, 0, 0, 5], [172, 20, 3, 4], [100, 101, 102, 103]] {
        let address = IpAddr::V4(Ipv4Addr::from(address));
        verify(&created, address).unwrap();
        verify(&reloaded, address).unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn name_constraints_stop_the_authority_vouching_for_public_addresses() {
    let root = directory();
    let tls = Tls::load_or_create(&root).unwrap();
    for address in [[8, 8, 8, 8], [172, 32, 0, 1], [1, 1, 1, 1]] {
        assert!(verify(&tls, IpAddr::V4(Ipv4Addr::from(address))).is_err(), "{address:?}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn a_client_trusting_the_authority_completes_an_encrypted_exchange() {
    let root = directory();
    let tls = Tls::load_or_create(&root).unwrap();
    let address = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));
    let acceptor = tokio_rustls::TlsAcceptor::from(tls.config_for(address).unwrap());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut client = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots(&tls))
        .with_no_client_auth();
    client.alpn_protocols = vec![b"http/1.1".to_vec()];
    let connector = tokio_rustls::TlsConnector::from(Arc::new(client));
    let (client_io, server_io) = tokio::io::duplex(16 * 1024);
    let server = tokio::spawn(async move {
        let mut stream = acceptor.accept(server_io).await.unwrap();
        let mut request = [0u8; 4];
        stream.read_exact(&mut request).await.unwrap();
        stream.write_all(b"pong").await.unwrap();
        stream.flush().await.unwrap();
        request
    });
    let mut stream = connector.connect(ServerName::IpAddress(address.into()), client_io).await.unwrap();
    assert_eq!(stream.get_ref().1.alpn_protocol(), Some(b"http/1.1".as_slice()));
    stream.write_all(b"ping").await.unwrap();
    let mut reply = [0u8; 4];
    stream.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"pong");
    assert_eq!(&server.await.unwrap(), b"ping");
    std::fs::remove_dir_all(root).unwrap();
}
