//! The PCs this phone is linked with, kept in the app's private storage.

use std::net::IpAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::pc::Endpoint;
use crate::tls::Trust;

/// One linked PC. Its certificate authority is its identity; its addresses are only where to look for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Computer {
    /// The authority's SHA-256 as hex.
    pub id: String,
    pub name: String,
    /// On its networks, the last address that answered first.
    pub addresses: Vec<IpAddr>,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    /// Where it answers from anywhere, through OpenTunnel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel: Option<Tunnel>,
    /// This phone's device session on the PC.
    pub token: String,
}

/// The PC's name through the tunnel, and the SHA-256 of the key its certificate must carry, as hex.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Tunnel {
    pub host: String,
    pub key: String,
}

impl Tunnel {
    pub(crate) fn from_pairing(tunnel: &drift_pairing::Tunnel) -> Self {
        Self {
            host: tunnel.host.clone(),
            key: drift_pairing::hex(&tunnel.key),
        }
    }
}

/// Where a PC can be reached now, as it reports it to a phone signed in to it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct Reach {
    #[serde(default)]
    pub addresses: Vec<IpAddr>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub tunnel: Option<Tunnel>,
}

impl Computer {
    pub(crate) fn fingerprint(&self) -> Option<crate::tls::Fingerprint> {
        drift_pairing::parse_fingerprint(&self.id)
    }

    /// Every way to reach it, its networks first, each with what the connection must trust.
    pub(crate) fn endpoints(&self) -> Vec<(Endpoint, Trust)> {
        let mut endpoints = Vec::new();
        if let Some(authority) = self.fingerprint() {
            for address in &self.addresses {
                let socket = std::net::SocketAddr::new(*address, self.port);
                endpoints.push((Endpoint::Lan(socket), Trust::Authority(authority)));
            }
        }
        let pinned = self.tunnel.as_ref().and_then(|tunnel| {
            let key = drift_pairing::parse_fingerprint(&tunnel.key)?;
            drift_pairing::valid_host(&tunnel.host).then(|| (Endpoint::Tunnel(tunnel.host.clone()), Trust::Key(key)))
        });
        endpoints.extend(pinned);

        endpoints
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    computers: Vec<Computer>,
    active: Option<String>,
}

pub(crate) struct Computers {
    path: PathBuf,
    saved: Saved,
}

impl Computers {
    /// What was saved at `path`; nothing saved, or a file this build cannot read, is an empty list.
    pub(crate) fn load(path: PathBuf) -> Self {
        let saved = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();

        Self { path, saved }
    }

    pub(crate) fn list(&self) -> &[Computer] {
        &self.saved.computers
    }

    pub(crate) fn get(&self, id: &str) -> Option<&Computer> {
        self.saved.computers.iter().find(|computer| computer.id == id)
    }

    pub(crate) fn active(&self) -> Option<&Computer> {
        self.get(self.saved.active.as_deref()?)
    }

    pub(crate) fn set_active(&mut self, id: Option<&str>) -> std::io::Result<()> {
        self.saved.active = id.map(str::to_string);
        self.save()
    }

    /// Adds a PC, or replaces it when linked again: its new token, plus any address it offers now.
    pub(crate) fn upsert(&mut self, mut computer: Computer) -> std::io::Result<()> {
        if let Some(index) = self.saved.computers.iter().position(|known| known.id == computer.id) {
            let old = self.saved.computers.remove(index);
            for address in old.addresses {
                if !computer.addresses.contains(&address) {
                    computer.addresses.push(address);
                }
            }
            computer.mac = computer.mac.or(old.mac);
            computer.tunnel = computer.tunnel.or(old.tunnel);
        }

        self.saved.computers.push(computer);
        self.save()
    }

    /// Takes what the PC reports over a connection that trusted it: its addresses now, with the one that answered
    /// first, and its tunnel, which it may have turned on or off since the phone last looked.
    pub(crate) fn refresh(&mut self, id: &str, answered: &Endpoint, reach: Reach) -> std::io::Result<()> {
        let Some(computer) = self.saved.computers.iter_mut().find(|computer| computer.id == id) else {
            return Ok(());
        };

        let mut addresses = reach.addresses;
        if let Endpoint::Lan(socket) = answered {
            addresses.retain(|address| *address != socket.ip());
            addresses.insert(0, socket.ip());
        }
        let refreshed = Computer {
            addresses: if addresses.is_empty() {
                computer.addresses.clone()
            } else {
                addresses
            },
            port: reach.port.unwrap_or(computer.port),
            tunnel: reach.tunnel,
            ..computer.clone()
        };
        if *computer == refreshed {
            return Ok(());
        }

        *computer = refreshed;
        self.save()
    }

    pub(crate) fn remove(&mut self, id: &str) -> std::io::Result<()> {
        self.saved.computers.retain(|computer| computer.id != id);
        if self.saved.active.as_deref() == Some(id) {
            self.saved.active = None;
        }

        self.save()
    }

    fn save(&self) -> std::io::Result<()> {
        if let Some(directory) = self.path.parent() {
            std::fs::create_dir_all(directory)?;
        }
        let bytes = serde_json::to_vec_pretty(&self.saved).map_err(std::io::Error::other)?;
        let staged = self.path.with_extension("json.new");
        std::fs::write(&staged, bytes)?;

        std::fs::rename(staged, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn computer(id: &str, addresses: &[&str]) -> Computer {
        Computer {
            id: id.into(),
            name: format!("PC {id}"),
            addresses: addresses.iter().map(|address| address.parse().unwrap()).collect(),
            port: 41718,
            mac: None,
            tunnel: None,
            token: format!("token {id}"),
        }
    }

    fn tunnel() -> Tunnel {
        Tunnel {
            host: "k2v7qxmd4hzt6wnc.x3fp8rk2mq7a.opentunnel.xyz".into(),
            key: "5a".repeat(32),
        }
    }

    #[test]
    fn linking_again_keeps_known_addresses_and_the_list_survives_a_restart() {
        let path = crate::test_dir().join(format!("drift-computers-{}.json", std::process::id()));
        let mut computers = Computers::load(path.clone());
        computers.upsert(computer("aa", &["192.168.1.20"])).unwrap();
        computers.upsert(computer("bb", &["10.0.0.5"])).unwrap();
        computers.set_active(Some("aa")).unwrap();

        let mut relinked = computer("aa", &["100.101.2.3"]);
        relinked.token = "fresh".into();
        relinked.tunnel = Some(tunnel());
        computers.upsert(relinked).unwrap();

        let reloaded = Computers::load(path.clone());
        let active = reloaded.active().unwrap();
        assert_eq!(active.token, "fresh");
        assert_eq!(active.tunnel, Some(tunnel()));
        assert_eq!(
            active.addresses,
            [
                "100.101.2.3".parse::<IpAddr>().unwrap(),
                "192.168.1.20".parse().unwrap()
            ]
        );
        assert_eq!(reloaded.list().len(), 2);

        let mut computers = reloaded;
        computers.remove("aa").unwrap();
        assert!(computers.active().is_none());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn what_the_pc_reports_replaces_its_addresses_and_tunnel_with_the_answering_address_first() {
        let path = crate::test_dir().join(format!("drift-refresh-{}.json", std::process::id()));
        let mut computers = Computers::load(path.clone());
        let mut known = computer("aa", &["192.168.1.20", "10.9.9.9"]);
        known.tunnel = Some(tunnel());
        computers.upsert(known).unwrap();

        let answered = Endpoint::Lan("100.101.2.3:41718".parse().unwrap());
        let reach = Reach {
            addresses: vec!["192.168.1.31".parse().unwrap(), "100.101.2.3".parse().unwrap()],
            port: Some(41718),
            tunnel: None,
        };
        computers.refresh("aa", &answered, reach).unwrap();

        let refreshed = Computers::load(path.clone()).get("aa").unwrap().clone();
        assert_eq!(
            refreshed.addresses,
            [
                "100.101.2.3".parse::<IpAddr>().unwrap(),
                "192.168.1.31".parse().unwrap()
            ]
        );
        assert_eq!(refreshed.tunnel, None, "the PC turned its tunnel off");

        computers
            .refresh(
                "aa",
                &Endpoint::Tunnel(tunnel().host),
                Reach {
                    tunnel: Some(tunnel()),
                    ..Reach::default()
                },
            )
            .unwrap();
        let away = computers.get("aa").unwrap();
        assert_eq!(
            away.addresses.len(),
            2,
            "a report without addresses keeps the known ones"
        );
        assert_eq!(away.tunnel, Some(tunnel()));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn endpoints_try_the_networks_first_and_pin_the_tunnel_to_its_key() {
        let mut linked = computer(&"ab".repeat(32), &["192.168.1.20"]);
        linked.tunnel = Some(tunnel());
        let endpoints = linked.endpoints();

        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0].0, Endpoint::Lan("192.168.1.20:41718".parse().unwrap()));
        assert_eq!(endpoints[0].1, Trust::Authority([0xab; 32]));
        assert_eq!(endpoints[1].0, Endpoint::Tunnel(tunnel().host));
        assert_eq!(endpoints[1].1, Trust::Key([0x5a; 32]));

        linked.tunnel = Some(Tunnel {
            host: "evil.example/steal".into(),
            key: "5a".repeat(32),
        });
        assert_eq!(linked.endpoints().len(), 1, "a damaged tunnel is never tried");
    }
}
