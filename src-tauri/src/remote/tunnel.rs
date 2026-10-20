//! Reaching this PC from anywhere through OpenTunnel, a relay that passes TLS through without reading it. The tunnel
//! client terminates TLS in this process and hands each connection to a loopback listener as plain HTTP, after a
//! PROXY v2 header naming the visitor (`docs/remote.md`).

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use axum::Router;
use axum::extract::{ConnectInfo, Request};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::service::TowerToHyperService;
use opentunnel::{Client, ClientOptions, Identity, ProxyProtocol, Route, State, Storage};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::Manager;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};
use tower::ServiceExt;

use super::RemoteAccess;

/// The one tunnel Drift keeps, under its own data directory rather than the shared XDG one, which Windows lacks.
const PROFILE: &str = "default";
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const PROXY_SIGNATURE: [u8; 12] = *b"\r\n\r\n\0\r\nQUIT\n";
/// A PROXY v2 header holding the visitor's address and the hostname it asked for is under 300 bytes.
const MAX_PROXY_BODY: usize = 1024;
const ROUTE_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
const ROUTE_LENGTH: usize = 16;

/// How the tunnel is doing, as Settings shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Phase {
    Off,
    SettingUp,
    Connecting,
    Online,
    Reconnecting,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TunnelStatus {
    pub(crate) phase: Phase,
    /// `https://<route>.<tunnel host>`, once the tunnel has its certificate.
    pub(crate) url: Option<String>,
    pub(crate) error: Option<String>,
}

impl TunnelStatus {
    pub(crate) const OFF: Self = Self {
        phase: Phase::Off,
        url: None,
        error: None,
    };
}

/// The host a request came through the tunnel for; the gateway refuses a request naming any other.
#[derive(Clone)]
pub(super) struct TunnelHost(pub(super) String);

/// The tunnel while it runs; stopping it closes the bridge and the loopback listener.
pub(super) struct Running {
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl Running {
    /// Whether it gave up for good, so turning the tunnel on again starts it afresh.
    pub(super) fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    pub(super) async fn stop(mut self) {
        let _ = self.shutdown.send(true);
        if tokio::time::timeout(Duration::from_secs(2), &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
        }
    }
}

/// Sets up the tunnel if this PC has none, then keeps `route` connected to `router` until stopped.
pub(super) fn start(app: tauri::AppHandle, router: Router, storage: PathBuf, route: String) -> Running {
    let (shutdown, stopped) = watch::channel(false);
    let task = tokio::spawn(run(app, router, storage, route, stopped));

    Running { shutdown, task }
}

async fn run(
    app: tauri::AppHandle,
    router: Router,
    storage: PathBuf,
    route: String,
    mut shutdown: watch::Receiver<bool>,
) {
    let access = app.state::<RemoteAccess>();
    let report = |phase, url: Option<&str>, error: Option<String>| {
        access.tunnel_changed(TunnelStatus {
            phase,
            url: url.map(str::to_string),
            error,
        });
    };

    // A new tunnel waits for its certificate, which can take a minute.
    report(Phase::SettingUp, None, None);
    let client = Client::new(ClientOptions {
        storage: Some(Storage::at(storage)),
        ..ClientOptions::default()
    });
    let identity = tokio::select! {
        _ = shutdown.changed() => return,
        identity = provision(&client, &report) => identity,
    };
    let opened = match identity {
        Ok(identity) => open(&client, &identity, &route, &router).await,
        Err(error) => Err(error),
    };
    let (tunnel, listener, endpoint) = match opened {
        Ok(opened) => opened,
        Err(error) => return report(Phase::Failed, None, Some(error)),
    };

    // Requests come in through the loopback listener; the tunnel's own state drives what Settings shows.
    let url = format!("https://{}", endpoint.host);
    access.set_tunnel_endpoint(Some(endpoint));
    let mut status = tunnel.status_watch();
    loop {
        let current = status.borrow_and_update().clone();
        let (phase, error) = phase_of(current.state, current.last_error);
        report(phase, Some(&url), error);
        if phase == Phase::Failed {
            break;
        }
        tokio::select! {
            _ = shutdown.changed() => break,
            changed = status.changed() => if changed.is_err() { break },
        }
    }

    access.set_tunnel_endpoint(None);
    listener.abort();
    let _ = tunnel.close().await;
}

/// Creates the tunnel, or finishes one an earlier run started, retrying what may pass and giving up on the rest.
async fn provision(client: &Client, report: &impl Fn(Phase, Option<&str>, Option<String>)) -> Result<Identity, String> {
    let mut delay = Duration::from_secs(1);
    loop {
        match client.ensure(PROFILE).await {
            Ok(identity) => return Ok(identity),
            Err(error) if error.is_transient() => {
                report(Phase::SettingUp, None, Some(error.to_string()));
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(30));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

type Opened = (opentunnel::Tunnel, JoinHandle<()>, drift_pairing::Tunnel);

/// Starts the loopback listener and claims `route` for it on the tunnel.
async fn open(client: &Client, identity: &Identity, route: &str, router: &Router) -> Result<Opened, String> {
    let key = certificate_key(&identity.certificate).ok_or("the tunnel's certificate could not be read")?;
    let host = format!("{route}.{}", identity.hostname);
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|error| error.to_string())?;
    let target = listener.local_addr().map_err(|error| error.to_string())?;

    let routes = [(
        route.to_string(),
        Route::new(target.to_string()).with_proxy_protocol(Some(ProxyProtocol::V2)),
    )]
    .into();
    let tunnel = client.connect(PROFILE, routes).map_err(|error| error.to_string())?;
    let serving = tokio::spawn(accept_loop(listener, router.clone(), host.clone()));

    Ok((tunnel, serving, drift_pairing::Tunnel { host, key }))
}

fn phase_of(state: State, error: Option<String>) -> (Phase, Option<String>) {
    match state {
        State::Connected => (Phase::Online, None),
        State::WaitingRoutes | State::Connecting => (Phase::Connecting, None),
        State::Reconnecting => (Phase::Reconnecting, error),
        State::Stopped => (
            Phase::Failed,
            Some(error.unwrap_or_else(|| "the tunnel stopped".into())),
        ),
    }
}

pub(super) async fn accept_loop(listener: TcpListener, router: Router, host: String) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                connections.spawn(serve(stream, router.clone(), host.clone()));
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

/// Serves one visitor's connection, known by the address in its PROXY header.
async fn serve(mut stream: TcpStream, router: Router, host: String) {
    let header = tokio::time::timeout(HEADER_TIMEOUT, read_proxy_header(&mut stream)).await;
    let Ok(Ok(visitor)) = header else { return };
    // A header without an address shares one throttling bucket rather than borrowing loopback's.
    let visitor = visitor.unwrap_or(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)));
    let service = router.map_request(move |mut request: Request<hyper::body::Incoming>| {
        request.extensions_mut().insert(ConnectInfo(visitor));
        request.extensions_mut().insert(TunnelHost(host.clone()));
        request
    });

    let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
        .serve_connection_with_upgrades(TokioIo::new(stream), TowerToHyperService::new(service))
        .await;
}

/// Reads the PROXY v2 header the tunnel client writes before a connection's data: the visitor's address, if named.
pub(super) async fn read_proxy_header(stream: &mut (impl AsyncReadExt + Unpin)) -> std::io::Result<Option<SocketAddr>> {
    let invalid = || std::io::Error::new(std::io::ErrorKind::InvalidData, "not a PROXY v2 header");
    let mut head = [0u8; 16];
    stream.read_exact(&mut head).await?;
    if head[..12] != PROXY_SIGNATURE || head[12] >> 4 != 2 {
        return Err(invalid());
    }
    let length = usize::from(u16::from_be_bytes([head[14], head[15]]));
    if length > MAX_PROXY_BODY {
        return Err(invalid());
    }

    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).await?;

    Ok(visitor(head[12] & 0x0f, head[13], &body))
}

/// The source address of a `PROXY` command over TCP; `LOCAL` and other families name none.
fn visitor(command: u8, family: u8, body: &[u8]) -> Option<SocketAddr> {
    const PROXY: u8 = 1;
    const TCP4: u8 = 0x11;
    const TCP6: u8 = 0x21;
    if command != PROXY {
        return None;
    }

    match family {
        TCP4 if body.len() >= 12 => {
            let address: [u8; 4] = body[..4].try_into().ok()?;
            Some(SocketAddr::from((
                Ipv4Addr::from(address),
                u16::from_be_bytes([body[8], body[9]]),
            )))
        }
        TCP6 if body.len() >= 36 => {
            let address: [u8; 16] = body[..16].try_into().ok()?;
            Some(SocketAddr::from((
                Ipv6Addr::from(address),
                u16::from_be_bytes([body[32], body[33]]),
            )))
        }
        _ => None,
    }
}

/// SHA-256 of the public key in the tunnel's certificate, which renewals keep: what the Drift app pins.
pub(super) fn certificate_key(pem: &str) -> Option<[u8; 32]> {
    let der = CertificateDer::pem_slice_iter(pem.as_bytes()).next()?.ok()?;
    let certificate = webpki::EndEntityCert::try_from(&der).ok()?;

    Some(Sha256::digest(certificate.subject_public_key_info().as_ref()).into())
}

/// A random route name: tunnel hostnames are public in certificate logs, but route names never appear there.
pub(super) fn new_route() -> String {
    let mut bytes = [0u8; ROUTE_LENGTH];
    getrandom::fill(&mut bytes).expect("the system random source works");

    bytes
        .iter()
        .map(|byte| char::from(ROUTE_ALPHABET[usize::from(*byte) % ROUTE_ALPHABET.len()]))
        .collect()
}
