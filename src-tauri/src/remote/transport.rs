use crate::remote_tls::Tls;
use axum::Router;
use axum::extract::{ConnectInfo, Request};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::service::TowerToHyperService;
use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

use super::HTTP_PORT;

const DISCOVERY_PROBE: &[u8] = b"OPENCODE_COMPANION_DISCOVERY";
const TLS_HANDSHAKE_RECORD: u8 = 0x16;
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DiscoveryDescriptor {
    kind: &'static str,
    name: &'static str,
    brand: &'static str,
    protocol: &'static str,
    version: u8,
    url: String,
    host: String,
    port: u16,
    certificate_sha256: String,
}

/// Serves HTTPS; dropping the connection set on shutdown aborts every open connection.
pub(super) async fn accept_loop(
    listener: TcpListener,
    router: Router,
    tls: Arc<Tls>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut connections = tokio::task::JoinSet::new();

    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            accepted = listener.accept() => {
                let Ok((stream, peer)) = accepted else { continue };
                connections.spawn(serve_connection(stream, peer, router.clone(), tls.clone()));
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

async fn serve_connection(stream: TcpStream, peer: SocketAddr, router: Router, tls: Arc<Tls>) {
    let mut first = [0u8; 1];
    let peeked = tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.peek(&mut first)).await;
    let Ok(local) = stream.local_addr() else { return };
    if !matches!(peeked, Ok(Ok(1))) || first[0] != TLS_HANDSHAKE_RECORD {
        return redirect_plain(stream, local).await;
    }

    let Ok(config) = tls.config_for(local.ip()) else { return };
    let accepted = tokio::time::timeout(HANDSHAKE_TIMEOUT, TlsAcceptor::from(config).accept(stream)).await;
    let Ok(Ok(stream)) = accepted else { return };
    let service = router.map_request(move |mut request: Request<hyper::body::Incoming>| {
        request.extensions_mut().insert(ConnectInfo(peer));
        request
    });

    let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
        .serve_connection_with_upgrades(TokioIo::new(stream), TowerToHyperService::new(service))
        .await;
}

/// Redirects plain-HTTP visitors, including typed addresses that default to HTTP, to the HTTPS origin.
async fn redirect_plain(mut stream: TcpStream, local: SocketAddr) {
    let mut head = vec![0u8; 8192];
    let Ok(Ok(read)) = tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.read(&mut head)).await else {
        return;
    };

    let response = plain_redirect(&String::from_utf8_lossy(&head[..read]), &local.to_string());
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

pub(super) fn plain_redirect(head: &str, fallback_host: &str) -> String {
    let safe = |value: &str| !value.is_empty() && value.chars().all(|character| character.is_ascii_graphic());
    let target = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .filter(|target| target.starts_with('/') && safe(target))
        .unwrap_or("/");
    let host = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("host"))
        .map(|(_, value)| value.trim())
        .filter(|host| safe(host) && !host.contains(['/', '\\', '@']))
        .unwrap_or(fallback_host);

    format!(
        concat!(
            "HTTP/1.1 308 Permanent Redirect\r\nLocation: https://{host}{target}\r\n",
            "Content-Length: 0\r\nConnection: close\r\n\r\n"
        ),
        host = host,
        target = target
    )
}

pub(super) async fn discovery_loop(
    socket: tokio::net::UdpSocket,
    fingerprint: String,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut buffer = [0u8; 256];

    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            received = socket.recv_from(&mut buffer) => {
                let Ok((size, peer)) = received else { continue };
                if &buffer[..size] != DISCOVERY_PROBE { continue; }
                let Some(ip) = local_ipv4_for(peer) else { continue };
                let descriptor = discovery_descriptor(ip, &fingerprint);

                if let Ok(payload) = serde_json::to_vec(&descriptor) {
                    let _ = socket.send_to(&payload, peer).await;
                }
            }
        }
    }
}

/// Version 2 uses HTTPS; clients may pin certificateSha256, the gateway CA fingerprint.
pub(super) fn discovery_descriptor(ip: Ipv4Addr, fingerprint: &str) -> DiscoveryDescriptor {
    DiscoveryDescriptor {
        kind: "drift-companion",
        name: "Drift",
        brand: "Drift",
        protocol: "drift-remote",
        version: 2,
        url: format!("https://{ip}:{HTTP_PORT}/companion"),
        host: ip.to_string(),
        port: HTTP_PORT,
        certificate_sha256: fingerprint.into(),
    }
}

fn local_ipv4_for(peer: SocketAddr) -> Option<Ipv4Addr> {
    let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(peer).ok()?;

    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_loopback() => Some(ip),
        _ => None,
    }
}

pub(super) fn local_ipv4() -> Option<Ipv4Addr> {
    local_ipv4_for(SocketAddr::from(([8, 8, 8, 8], 53)))
}
