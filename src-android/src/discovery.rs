//! Finds Drift PCs on this network by the probe the gateway answers on UDP 41717.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use serde::{Deserialize, Serialize};

const PROBE: &[u8] = b"OPENCODE_COMPANION_DISCOVERY";
const DISCOVERY_PORT: u16 = 41717;

/// A PC that answered. Discovery is unauthenticated, so its fingerprint only labels it; linking never trusts it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Found {
    pub name: String,
    pub address: IpAddr,
    pub port: u16,
    pub fingerprint: String,
    pub api: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Answer {
    kind: String,
    host: String,
    port: u16,
    #[serde(default)]
    computer: String,
    #[serde(default)]
    certificate_sha256: String,
    #[serde(default)]
    api: u32,
}

/// Broadcasts the probe and collects answers for `wait`, one per PC.
pub(crate) async fn discover(wait: Duration) -> Vec<Found> {
    let Ok(socket) = tokio::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await else {
        return Vec::new();
    };
    if socket.set_broadcast(true).is_err()
        || socket
            .send_to(PROBE, SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT)))
            .await
            .is_err()
    {
        return Vec::new();
    }

    let mut found = Vec::new();
    let mut seen = HashSet::new();
    let deadline = tokio::time::Instant::now() + wait;
    let mut buffer = [0u8; 2048];
    while let Ok(Ok((size, from))) = tokio::time::timeout_at(deadline, socket.recv_from(&mut buffer)).await {
        let Some(answer) = parse(&buffer[..size], from.ip()) else {
            continue;
        };
        if seen.insert(answer.fingerprint.clone()) {
            found.push(answer);
        }
    }

    found
}

fn parse(bytes: &[u8], from: IpAddr) -> Option<Found> {
    let answer: Answer = serde_json::from_slice(bytes).ok()?;
    if answer.kind != "drift-companion" {
        return None;
    }
    let address = answer.host.parse().unwrap_or(from);
    let fingerprint = drift_pairing::parse_fingerprint(&answer.certificate_sha256)?;

    Some(Found {
        name: if answer.computer.is_empty() {
            address.to_string()
        } else {
            answer.computer
        },
        address,
        port: answer.port,
        fingerprint: crate::pc::hex(&fingerprint),
        api: answer.api,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gateways_answer_reads_as_a_found_pc_and_anything_else_is_ignored() {
        let fingerprint = vec!["AB"; 32].join(":");
        let answer = serde_json::json!({
            "kind": "drift-companion", "host": "192.168.1.20", "port": 41718,
            "computer": "DESKTOP", "certificateSha256": fingerprint, "api": 1
        });
        let found = parse(answer.to_string().as_bytes(), "192.168.1.20".parse().unwrap()).unwrap();

        assert_eq!(found.name, "DESKTOP");
        assert_eq!(found.fingerprint, "ab".repeat(32));
        assert_eq!((found.port, found.api), (41718, 1));
        assert!(parse(b"{\"kind\":\"other\"}", "10.0.0.1".parse().unwrap()).is_none());
        assert!(parse(b"not json", "10.0.0.1".parse().unwrap()).is_none());
    }
}
