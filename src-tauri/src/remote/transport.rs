use crate::remote_tls::Tls;
use axum::Router;
use axum::extract::{ConnectInfo, Request};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::service::TowerToHyperService;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

const TLS_HANDSHAKE_RECORD: u8 = 0x16;
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

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

/// The address this PC uses on its LAN: the one its route to the internet leaves from.
pub(super) fn local_ipv4() -> Option<Ipv4Addr> {
    let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(SocketAddr::from(([8, 8, 8, 8], 53))).ok()?;

    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_loopback() => Some(ip),
        _ => None,
    }
}
