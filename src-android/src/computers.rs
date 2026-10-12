//! The PCs this phone is linked with, kept in the app's private storage.

use std::net::IpAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::tls::Fingerprint;

/// One linked PC. Its certificate authority is its identity; its addresses are only where to look for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Computer {
    /// The authority's SHA-256 as hex.
    pub id: String,
    pub name: String,
    /// The last address that answered first.
    pub addresses: Vec<IpAddr>,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    /// This phone's device session on the PC.
    pub token: String,
}

impl Computer {
    pub(crate) fn fingerprint(&self) -> Option<Fingerprint> {
        drift_pairing::parse_fingerprint(&self.id)
    }

    pub(crate) fn words(&self) -> Vec<&'static str> {
        self.fingerprint()
            .map(|fingerprint| drift_pairing::words(&fingerprint))
            .unwrap_or_default()
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
        }

        self.saved.computers.push(computer);
        self.save()
    }

    /// Puts the address that answered first, so the next connection tries it before the others.
    pub(crate) fn answered(&mut self, id: &str, address: IpAddr) -> std::io::Result<()> {
        let Some(computer) = self.saved.computers.iter_mut().find(|computer| computer.id == id) else {
            return Ok(());
        };
        if computer.addresses.first() == Some(&address) {
            return Ok(());
        }

        computer.addresses.retain(|known| *known != address);
        computer.addresses.insert(0, address);
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
            token: format!("token {id}"),
        }
    }

    #[test]
    fn linking_again_keeps_known_addresses_and_the_list_survives_a_restart() {
        let path = std::env::temp_dir().join(format!("drift-computers-{}.json", std::process::id()));
        let mut computers = Computers::load(path.clone());
        computers.upsert(computer("aa", &["192.168.1.20"])).unwrap();
        computers.upsert(computer("bb", &["10.0.0.5"])).unwrap();
        computers.set_active(Some("aa")).unwrap();

        let mut relinked = computer("aa", &["100.101.2.3"]);
        relinked.token = "fresh".into();
        computers.upsert(relinked).unwrap();
        computers.answered("aa", "192.168.1.20".parse().unwrap()).unwrap();

        let reloaded = Computers::load(path.clone());
        let active = reloaded.active().unwrap();
        assert_eq!(active.token, "fresh");
        assert_eq!(
            active.addresses,
            [
                "192.168.1.20".parse::<IpAddr>().unwrap(),
                "100.101.2.3".parse().unwrap()
            ]
        );
        assert_eq!(reloaded.list().len(), 2);

        let mut computers = reloaded;
        computers.remove("aa").unwrap();
        assert!(computers.active().is_none());
        std::fs::remove_file(path).ok();
    }
}
