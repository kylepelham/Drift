//! Talking to one PC's Remote Access gateway: its version, asking it to connect, signing out, and finding it again. A
//! PC is tried on every network address it has and through its tunnel at once; the network wins when both answer.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;

use crate::computers::{Computer, Reach};
use crate::tls::{self, Trust};

/// How long a network address may still answer once the tunnel has: home is faster than the relay.
const NETWORK_GRACE: Duration = Duration::from_millis(400);

#[derive(Debug, thiserror::Error)]
pub enum PcError {
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
    #[error("this device was signed out of {0}; link it again")]
    SignedOut(String),
    #[error("{0}")]
    Refused(String),
    #[error("connected, but this device could not save it for next time: {0}")]
    Unsaved(std::io::Error),
    #[error("{0} declined the connection")]
    Declined(String),
    #[error("nobody accepted the connection on {0} in time; try again")]
    Expired(String),
    #[error("{0} is not an address this device can find")]
    Address(String),
}

/// What the gateway tells a client that has not signed in.
#[derive(Debug, Deserialize)]
pub struct Options {
    #[serde(default)]
    pub api: u32,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub computer: String,
}

/// A PC asked to accept this device: the request it waits on, and the code both screens show meanwhile.
#[derive(Clone, Debug)]
pub struct Asking {
    pub pc: Pc,
    pub name: String,
    pub code: String,
    request: String,
    authority: tls::Fingerprint,
}

/// The signed-in device as the gateway describes it; only where the PC can be reached matters here.
#[derive(Deserialize)]
struct Me {
    #[serde(default)]
    reach: Option<Reach>,
}

/// Where a PC is reached: an address on one of its networks, or its name through OpenTunnel.
#[derive(Clone, Debug, PartialEq)]
pub enum Endpoint {
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
pub struct Pc {
    pub client: reqwest::Client,
    pub base: String,
    pub endpoint: Endpoint,
}

impl Pc {
    pub fn new(endpoint: Endpoint, trust: Trust) -> Self {
        Self {
            client: tls::client(trust),
            base: endpoint.base(),
            endpoint,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn unreachable(&self) -> PcError {
        PcError::Unreachable(self.base.clone())
    }

    /// The gateway's options, refusing a PC whose remote API this app does not speak.
    pub async fn options(&self) -> Result<Options, PcError> {
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

    /// Where the PC says it can be reached now, or `None` when it signed this device out.
    pub async fn reach(&self, token: &str) -> Result<Option<Reach>, PcError> {
        let response = self
            .client
            .get(self.url("/auth/me"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| self.unreachable())?;
        match response.status() {
            StatusCode::UNAUTHORIZED => return Ok(None),
            // A PC from before it reported this has no such route; this device keeps what it knew.
            StatusCode::NOT_FOUND => return Ok(Some(Reach::default())),
            status if !status.is_success() => return Err(self.unreachable()),
            _ => {}
        }

        let me: Me = response.json().await.map_err(|_| self.unreachable())?;
        Ok(Some(me.reach.unwrap_or_default()))
    }

    /// A request to a PC not linked yet, answered with JSON, or its refusal. A PC from before devices asked to connect
    /// has no such route.
    async fn post(&self, path: &str, body: &serde_json::Value, name: &str) -> Result<serde_json::Value, PcError> {
        let response = self
            .client
            .post(self.url(path))
            .json(body)
            .send()
            .await
            .map_err(|_| PcError::Unreachable(name.into()))?;
        let status = response.status();
        let body: serde_json::Value = response.json().await.unwrap_or_default();
        if status == StatusCode::NOT_FOUND {
            return Err(PcError::TooOld {
                name: name.into(),
                version: "from before devices asked to connect".into(),
            });
        }
        if !status.is_success() {
            let refusal = body["error"].as_str().unwrap_or("the connection was refused");
            return Err(PcError::Refused(refusal.into()));
        }

        Ok(body)
    }

    /// Signs this device out of the PC; the PC forgets it either way the call ends.
    pub async fn sign_out(&self, token: &str) {
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

/// Where a typed address points: an IP or a name, with the gateway's port unless it names another.
pub async fn resolve(typed: &str) -> Result<SocketAddr, PcError> {
    let typed = typed.trim();
    let invalid = || PcError::Address(typed.to_string());
    if let Ok(socket) = typed.parse::<SocketAddr>() {
        return Ok(socket);
    }
    if let Ok(address) = typed.trim_matches(['[', ']']).parse::<IpAddr>() {
        return Ok(SocketAddr::new(address, drift_pairing::PORT));
    }
    let (host, port) = match typed.rsplit_once(':') {
        Some((host, port)) => (host, port.parse().map_err(|_| invalid())?),
        None => (typed, drift_pairing::PORT),
    };
    if host.is_empty() || host.contains(['/', '@', ' ']) {
        return Err(invalid());
    }

    let found: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| invalid())?
        .collect();
    found
        .iter()
        .find(|socket| socket.is_ipv4())
        .or(found.first())
        .copied()
        .ok_or_else(invalid)
}

/// Asks the PC at `address` to accept this device, trusting the authority it shows. Each side then reveals a secret,
/// the PC having committed to its own first, and both derive the code its user compares before accepting.
pub async fn ask(address: SocketAddr, device: &str, this_computer: Option<&str>) -> Result<Asking, PcError> {
    let (client, met) = tls::first_use();
    let pc = Pc {
        client,
        base: Endpoint::Lan(address).base(),
        endpoint: Endpoint::Lan(address),
    };
    let options = pc.options().await?;
    let name = if options.computer.is_empty() {
        address.ip().to_string()
    } else {
        options.computer
    };
    let not_drift = || PcError::Refused(format!("{name} did not answer as Drift does"));
    let authority = met.get().copied().ok_or_else(not_drift)?;
    if this_computer == Some(hex(&authority).as_str()) {
        return Err(PcError::Refused("that is this computer".into()));
    }

    let opened = pc.post("/auth/request", &json!({ "name": device }), &name).await?;
    let request = opened["id"].as_str().ok_or_else(not_drift)?;
    let commitment = opened["commitment"]
        .as_str()
        .and_then(drift_pairing::parse_fingerprint)
        .ok_or_else(not_drift)?;

    let secret: drift_pairing::Secret = random();
    let revealed = pc
        .post(
            "/auth/request/reveal",
            &json!({ "id": request, "secret": hex(&secret) }),
            &name,
        )
        .await?;
    let theirs = revealed["secret"]
        .as_str()
        .and_then(drift_pairing::parse_fingerprint)
        .ok_or_else(not_drift)?;
    if drift_pairing::commitment(&theirs) != commitment {
        return Err(PcError::Refused(format!(
            "{name} changed its answer midway; something on this network is interfering"
        )));
    }

    Ok(Asking {
        code: drift_pairing::matching_code(&authority, &secret, &theirs),
        pc,
        name,
        request: request.into(),
        authority,
    })
}

/// Waits for the PC's user to answer; accepted, the PC linked with this device's session and where it is reached.
pub async fn answer(asking: &Asking) -> Result<(Computer, Pc), PcError> {
    let token = loop {
        let response = asking
            .pc
            .client
            .post(asking.pc.url("/auth/request/wait"))
            .json(&json!({ "id": asking.request }))
            .send()
            .await
            .map_err(|_| PcError::Unreachable(asking.name.clone()))?;
        let status = response.status();
        let body: serde_json::Value = response.json().await.unwrap_or_default();
        match status {
            StatusCode::FORBIDDEN => return Err(PcError::Declined(asking.name.clone())),
            StatusCode::GONE => return Err(PcError::Expired(asking.name.clone())),
            _ => {}
        }
        if let Some(token) = body["token"].as_str() {
            break token.to_string();
        }
        if body["waiting"] != true {
            let refusal = body["error"].as_str().unwrap_or("the connection was refused");
            return Err(PcError::Refused(refusal.into()));
        }
    };

    let Endpoint::Lan(socket) = asking.pc.endpoint else {
        unreachable!("a first link is made on the PC's network")
    };
    let pc = Pc::new(Endpoint::Lan(socket), Trust::Authority(asking.authority));
    let reach = pc.reach(&token).await.ok().flatten().unwrap_or_default();
    let mut addresses = reach.addresses;
    addresses.retain(|address| *address != socket.ip());
    addresses.insert(0, socket.ip());
    let computer = Computer {
        id: hex(&asking.authority),
        name: asking.name.clone(),
        addresses,
        port: socket.port(),
        mac: reach.mac,
        tunnel: reach.tunnel,
        token,
    };

    Ok((computer, pc))
}

/// Tells the PC this device stopped waiting, so its user is no longer asked.
pub async fn cancel(asking: &Asking) {
    let _ = asking
        .pc
        .client
        .post(asking.pc.url("/auth/request/cancel"))
        .json(&json!({ "id": asking.request }))
        .send()
        .await;
}

fn random() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the system random source works");
    bytes
}

pub use drift_pairing::hex;

/// Finds a linked PC wherever it answers, and what it reports about where it can be reached now.
pub async fn reach(computer: &Computer) -> Result<(Pc, Reach), PcError> {
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
