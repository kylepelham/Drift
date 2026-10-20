//! Talking to one PC's Remote Access gateway: its version, pairing, signing out, and finding it again. A PC is
//! tried on every network address it has and through its tunnel at once; the network wins when both answer.

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;

use crate::computers::{Computer, Reach, Tunnel};
use crate::tls::{self, Trust};

/// How long a network address may still answer once the tunnel has: home is faster than the relay.
const NETWORK_GRACE: Duration = Duration::from_millis(400);

#[derive(Debug, thiserror::Error)]
pub(crate) enum PcError {
    #[error("{0} could not be reached; check that it is on and that Remote Access is turned on")]
    Unreachable(String),
    #[error(
        "{0} could not be reached on this network. To reach it from anywhere, turn on Reach from anywhere in its \
         Remote Access settings, then open this app once on its network"
    )]
    NetworkOnly(String),
    #[error("{name} runs Drift {version}, which this app cannot drive; update Drift on {name}")]
    TooOld { name: String, version: String },
    #[error("{name} runs Drift {version}, which needs a newer Drift app; update this app")]
    TooNew { name: String, version: String },
    #[error("this phone was signed out of {0}; link it again")]
    SignedOut(String),
    #[error("{0}")]
    Refused(String),
    #[error(transparent)]
    Pairing(#[from] drift_pairing::PairingError),
}

/// What the gateway tells a client that has not signed in.
#[derive(Debug, Deserialize)]
pub(crate) struct Options {
    #[serde(default)]
    pub api: u32,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub computer: String,
}

/// The signed-in device as the gateway describes it; only where the PC can be reached matters here.
#[derive(Deserialize)]
struct Me {
    #[serde(default)]
    reach: Option<Reach>,
}

/// Where a PC is reached: an address on one of its networks, or its name through OpenTunnel.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Endpoint {
    Lan(SocketAddr),
    Tunnel(String),
}

impl Endpoint {
    fn base(&self) -> String {
        match self {
            Self::Lan(socket) => format!("https://{socket}"),
            Self::Tunnel(host) => format!("https://{host}"),
        }
    }

    fn on_network(&self) -> bool {
        matches!(self, Self::Lan(_))
    }
}

/// One PC at one endpoint, over a connection that trusts only that PC.
#[derive(Clone, Debug)]
pub(crate) struct Pc {
    pub client: reqwest::Client,
    pub base: String,
    pub endpoint: Endpoint,
}

impl Pc {
    pub(crate) fn new(endpoint: Endpoint, trust: Trust) -> Self {
        Self {
            client: tls::client(trust),
            base: endpoint.base(),
            endpoint,
        }
    }

    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn unreachable(&self) -> PcError {
        PcError::Unreachable(self.base.clone())
    }

    /// The gateway's options, refusing a PC whose remote API this app does not speak.
    pub(crate) async fn options(&self) -> Result<Options, PcError> {
        let options: Options = self
            .client
            .get(self.url("/auth/options"))
            .send()
            .await
            .map_err(|_| self.unreachable())?
            .json()
            .await
            .map_err(|_| self.unreachable())?;

        let name = if options.computer.is_empty() {
            self.base.clone()
        } else {
            options.computer.clone()
        };
        let version = options.version.clone();
        match options.api.cmp(&drift_pairing::API_VERSION) {
            std::cmp::Ordering::Less => Err(PcError::TooOld { name, version }),
            std::cmp::Ordering::Greater => Err(PcError::TooNew { name, version }),
            std::cmp::Ordering::Equal => Ok(options),
        }
    }

    /// Where the PC says it can be reached now, or `None` when it signed this phone out.
    pub(crate) async fn reach(&self, token: &str) -> Result<Option<Reach>, PcError> {
        let response = self
            .client
            .get(self.url("/auth/me"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| self.unreachable())?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Ok(None);
        }

        // A PC from before it reported this answers without it; the phone keeps what it knew.
        let me: Me = response.json().await.unwrap_or(Me { reach: None });
        Ok(Some(me.reach.unwrap_or_default()))
    }

    /// Signs this phone out of the PC; the PC forgets it either way the call ends.
    pub(crate) async fn sign_out(&self, token: &str) {
        let _ = self
            .client
            .post(self.url("/auth/logout"))
            .bearer_auth(token)
            .send()
            .await;
    }
}

/// The first endpoint whose gateway answers, preferring a network address over the tunnel.
async fn first_answering(candidates: Vec<(Endpoint, Trust)>) -> Result<(Pc, Options), PcError> {
    let mut attempts: FuturesUnordered<_> = candidates
        .into_iter()
        .map(|(endpoint, trust)| async move {
            let pc = Pc::new(endpoint, trust);
            let options = pc.options().await;
            (pc, options)
        })
        .collect();

    let mut through_tunnel = None;
    let mut deadline = None;
    let mut refusal = None;
    loop {
        let next = match deadline {
            Some(deadline) => match tokio::time::timeout_at(deadline, attempts.next()).await {
                Ok(next) => next,
                Err(_) => break,
            },
            None => attempts.next().await,
        };
        match next {
            None => break,
            Some((pc, Ok(options))) if pc.endpoint.on_network() => return Ok((pc, options)),
            Some((pc, Ok(options))) => {
                through_tunnel = Some((pc, options));
                deadline = Some(tokio::time::Instant::now() + NETWORK_GRACE);
            }
            Some((_, Err(PcError::Unreachable(_)))) => {}
            Some((_, Err(error))) => refusal = Some(error),
        }
    }

    match (through_tunnel, refusal) {
        (Some(answered), _) => Ok(answered),
        (None, Some(refusal)) => Err(refusal),
        (None, None) => Err(PcError::Unreachable(String::new())),
    }
}

/// Redeems a scanned pairing code at whichever of its endpoints answers; the ticket works once, so only there.
pub(crate) async fn pair(pairing: &drift_pairing::Pairing, device: &str) -> Result<(Computer, Pc), PcError> {
    let mut candidates: Vec<(Endpoint, Trust)> = pairing
        .addresses
        .iter()
        .map(|address| {
            let socket = SocketAddr::new(*address, pairing.port);
            (Endpoint::Lan(socket), Trust::Authority(pairing.fingerprint))
        })
        .collect();
    if let Some(tunnel) = &pairing.tunnel {
        candidates.push((Endpoint::Tunnel(tunnel.host.clone()), Trust::Key(tunnel.key)));
    }
    let (pc, options) = first_answering(candidates).await.map_err(|error| match error {
        PcError::Unreachable(_) => PcError::Unreachable(pairing.name.clone()),
        error => error,
    })?;

    let response = pc
        .client
        .post(pc.url("/auth/pair"))
        .json(&json!({ "ticket": pairing.ticket, "name": device }))
        .send()
        .await
        .map_err(|_| PcError::Unreachable(pairing.name.clone()))?;
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    let Some(token) = body["token"].as_str() else {
        let refusal = body["error"].as_str().unwrap_or("the PC refused the pairing code");
        return Err(PcError::Refused(refusal.into()));
    };

    let mut addresses = pairing.addresses.clone();
    if let Endpoint::Lan(socket) = &pc.endpoint {
        addresses.retain(|address| *address != socket.ip());
        addresses.insert(0, socket.ip());
    }
    let computer = Computer {
        id: hex(&pairing.fingerprint),
        name: if options.computer.is_empty() {
            pairing.name.clone()
        } else {
            options.computer
        },
        addresses,
        port: pairing.port,
        mac: pairing.mac.map(|mac| hex(&mac)),
        tunnel: pairing.tunnel.as_ref().map(Tunnel::from_pairing),
        token: token.into(),
    };

    Ok((computer, pc))
}

pub(crate) use drift_pairing::hex;

/// Finds a linked PC wherever it answers, and what it reports about where it can be reached now.
pub(crate) async fn reach(computer: &Computer) -> Result<(Pc, Reach), PcError> {
    let unreachable = || match computer.tunnel {
        Some(_) => PcError::Unreachable(computer.name.clone()),
        None => PcError::NetworkOnly(computer.name.clone()),
    };
    let (pc, _) = first_answering(computer.endpoints())
        .await
        .map_err(|error| match error {
            PcError::Unreachable(_) => unreachable(),
            error => error,
        })?;

    match pc.reach(&computer.token).await {
        Ok(Some(reach)) => Ok((pc, reach)),
        Ok(None) => Err(PcError::SignedOut(computer.name.clone())),
        Err(_) => Err(unreachable()),
    }
}
