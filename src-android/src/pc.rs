//! Talking to one PC's Remote Access gateway: its version, pairing, linking by code, signing out.

use std::net::{IpAddr, SocketAddr};

use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;

use crate::computers::Computer;
use crate::tls::{self, Fingerprint, Trusted};

#[derive(Debug, thiserror::Error)]
pub(crate) enum PcError {
    #[error("{0} could not be reached; check that it is on and that Remote Access is turned on")]
    Unreachable(String),
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

/// One PC at one address, over a connection that trusts only its authority.
#[derive(Clone, Debug)]
pub(crate) struct Pc {
    pub client: reqwest::Client,
    pub base: String,
    pub address: IpAddr,
    pub port: u16,
    pub trusted: Trusted,
}

impl Pc {
    pub(crate) fn new(fingerprint: Option<Fingerprint>, address: IpAddr, port: u16) -> Self {
        let (client, trusted) = tls::client(fingerprint);

        Self {
            client,
            base: format!("https://{}", SocketAddr::new(address, port)),
            address,
            port,
            trusted,
        }
    }

    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// The gateway's options, refusing a PC whose remote API this app does not speak.
    pub(crate) async fn options(&self) -> Result<Options, PcError> {
        let unreachable = |_| PcError::Unreachable(self.address.to_string());
        let options: Options = self
            .client
            .get(self.url("/auth/options"))
            .send()
            .await
            .map_err(unreachable)?
            .json()
            .await
            .map_err(unreachable)?;

        let name = if options.computer.is_empty() {
            self.address.to_string()
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

    /// Whether this phone's session on the PC still stands.
    pub(crate) async fn signed_in(&self, token: &str) -> Result<bool, PcError> {
        let response = self
            .client
            .get(self.url("/auth/me"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| PcError::Unreachable(self.address.to_string()))?;

        Ok(response.status() != StatusCode::UNAUTHORIZED)
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

/// Redeems a scanned pairing code at the first of its addresses that answers.
pub(crate) async fn pair(pairing: &drift_pairing::Pairing, device: &str) -> Result<(Computer, Pc), PcError> {
    let mut last = PcError::Unreachable(pairing.name.clone());
    for address in &pairing.addresses {
        let pc = Pc::new(Some(pairing.fingerprint), *address, pairing.port);
        match redeem(&pc, pairing, device).await {
            Ok(computer) => return Ok((computer, pc)),
            Err(error @ PcError::Unreachable(_)) => last = error,
            Err(error) => return Err(error),
        }
    }

    Err(last)
}

async fn redeem(pc: &Pc, pairing: &drift_pairing::Pairing, device: &str) -> Result<Computer, PcError> {
    let options = pc.options().await?;
    let response = pc
        .client
        .post(pc.url("/auth/pair"))
        .json(&json!({ "ticket": pairing.ticket, "name": device }))
        .send()
        .await
        .map_err(|_| PcError::Unreachable(pc.address.to_string()))?;
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    let Some(token) = body["token"].as_str() else {
        let refusal = body["error"].as_str().unwrap_or("the PC refused the pairing code");
        return Err(PcError::Refused(refusal.into()));
    };

    let name = if options.computer.is_empty() {
        pairing.name.clone()
    } else {
        options.computer
    };
    let mut addresses = vec![pc.address];
    addresses.extend(pairing.addresses.iter().filter(|address| **address != pc.address));

    Ok(Computer {
        id: hex(&pairing.fingerprint),
        name,
        addresses,
        port: pairing.port,
        mac: pairing.mac.map(|mac| hex(&mac)),
        token: token.into(),
    })
}

/// A link by typed code under way: the code to type on the PC and the authority to compare by its words.
pub(crate) struct Linking {
    pub pc: Pc,
    pub name: String,
    pub id: String,
    pub code: String,
    pub authority: Fingerprint,
}

pub(crate) enum Linked {
    Waiting,
    Expired,
    Approved(Computer),
}

/// Asks the PC at `address` for a code the user types on the PC. The authority met now is held from here on.
pub(crate) async fn start_link(address: IpAddr, port: u16, device: &str) -> Result<Linking, PcError> {
    let pc = Pc::new(None, address, port);
    let options = pc.options().await?;
    let response = pc
        .client
        .post(pc.url("/auth/link"))
        .json(&json!({ "name": device }))
        .send()
        .await
        .map_err(|_| PcError::Unreachable(address.to_string()))?;
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    let (Some(id), Some(code)) = (body["id"].as_str(), body["code"].as_str()) else {
        let refusal = body["error"].as_str().unwrap_or("the PC refused to link");
        return Err(PcError::Refused(refusal.into()));
    };
    let authority = pc
        .trusted
        .get()
        .ok_or_else(|| PcError::Unreachable(address.to_string()))?;

    Ok(Linking {
        name: if options.computer.is_empty() {
            address.to_string()
        } else {
            options.computer
        },
        id: id.into(),
        code: code.into(),
        authority,
        pc,
    })
}

/// Whether the code has been typed on the PC yet; once it has, the PC's session cookie is this phone's token.
pub(crate) async fn poll_link(linking: &Linking) -> Result<Linked, PcError> {
    let response = linking
        .pc
        .client
        .get(linking.pc.url(&format!("/auth/link/{}", linking.id)))
        .send()
        .await
        .map_err(|_| PcError::Unreachable(linking.pc.address.to_string()))?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(Linked::Expired);
    }

    let token = response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(session_token);
    let Some(token) = token else {
        return Ok(Linked::Waiting);
    };

    Ok(Linked::Approved(Computer {
        id: hex(&linking.authority),
        name: linking.name.clone(),
        addresses: vec![linking.pc.address],
        port: linking.pc.port,
        mac: None,
        token,
    }))
}

/// The gateway's `drift_remote` session cookie, which a linked browser keeps and this app sends as a bearer token.
fn session_token(cookie: &str) -> Option<String> {
    let (name, rest) = cookie.split_once('=')?;
    let value = rest.split(';').next()?.trim();

    (name.trim() == "drift_remote" && !value.is_empty()).then(|| value.to_string())
}

pub(crate) use drift_pairing::hex;

/// Finds the address of a linked PC that answers, trying the last one that did first.
pub(crate) async fn reach(computer: &Computer) -> Result<Pc, PcError> {
    let fingerprint = computer.fingerprint();
    for address in &computer.addresses {
        let pc = Pc::new(fingerprint, *address, computer.port);
        match pc.options().await {
            Ok(_) if pc.signed_in(&computer.token).await? => return Ok(pc),
            Ok(_) => return Err(PcError::SignedOut(computer.name.clone())),
            Err(PcError::Unreachable(_)) => {}
            Err(error) => return Err(error),
        }
    }

    Err(PcError::Unreachable(computer.name.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_cookie_is_read_from_the_link_answer() {
        assert_eq!(
            session_token("drift_remote=abc123; HttpOnly; Secure; SameSite=Strict; Path=/").as_deref(),
            Some("abc123")
        );
        assert_eq!(session_token("other=1; Path=/"), None);
        assert_eq!(session_token("drift_remote=; Max-Age=0"), None);
    }
}
