//! What a Drift computer and a device that drives it agree on to link: how the device finds computers nearby, the
//! code both screens show while the computer's user accepts or declines it, and the remote API version. Neither
//! side's engine or UI is in here.
//!
//! Linking is first-use: the device trusts the certificate authority it met, so a device in the middle of the
//! connection could relay the request and hold the session the user accepts. The code is what stops it. Each side
//! puts in a random secret, the computer committing to its own before it sees the device's, and the code is a hash of
//! both and of the authority the device met. A relay meets the device with an authority of its own; to make the two
//! codes agree it would have to choose a secret knowing the other side's, and the order the secrets are revealed in
//! never lets it.

use std::fmt::Write as _;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The remote API the computer's gateway serves; a device refuses a computer whose major version differs.
pub const API_VERSION: u32 = 1;

/// The gateway's port over TCP, and over UDP where it answers devices looking for computers nearby.
pub const PORT: u16 = 41718;

/// What a device broadcasts to find computers nearby.
pub const DISCOVER: &[u8] = b"drift-discover/1";

/// How a computer with Remote Access on answers `DISCOVER`, from the address it is reached at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Announcement {
    pub name: String,
    /// SHA-256 of its certificate authority, as hex: which computer it is, so a linked one is recognized.
    pub id: String,
    pub port: u16,
    pub api: u32,
}

/// A random value one side of a link contributes to its code.
pub type Secret = [u8; 32];

/// Where the computer answers over the internet, through OpenTunnel, and the key its certificate must carry.
#[derive(Clone, Debug, PartialEq)]
pub struct Tunnel {
    /// The computer's own name under the tunnel, such as `k2v7.abcd1234efgh.opentunnel.xyz`.
    pub host: String,
    /// SHA-256 of the certificate's public key. The relay could have a certificate issued for the host, but not
    /// one for this key, which never leaves the computer.
    pub key: [u8; 32],
}

/// What the computer sends before it learns the device's secret, binding it to its own.
pub fn commitment(secret: &Secret) -> [u8; 32] {
    Sha256::new()
        .chain_update(b"drift-commitment/1")
        .chain_update(secret)
        .finalize()
        .into()
}

/// The six digits both screens show, written as two groups of three.
pub fn matching_code(authority: &[u8; 32], device: &Secret, computer: &Secret) -> String {
    let digest = Sha256::new()
        .chain_update(b"drift-code/1")
        .chain_update(authority)
        .chain_update(device)
        .chain_update(computer)
        .finalize();
    let number = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 1_000_000;

    format!("{:03} {:03}", number / 1000, number % 1000)
}

/// Addresses a first link may come from: this network, the machine itself, and Tailscale. Never the internet.
pub fn on_local_network(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let [first, second, ..] = address.octets();
            let tailscale = first == 100 && (64..128).contains(&second);
            address.is_private() || address.is_loopback() || address.is_link_local() || tailscale
        }
        IpAddr::V6(address) => match address.to_ipv4_mapped() {
            Some(mapped) => on_local_network(IpAddr::V4(mapped)),
            None => {
                let first = address.segments()[0];
                address.is_loopback() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
            }
        },
    }
}

/// A DNS name of plain labels, so a computer cannot point a device at a URL with a path, port or credentials.
pub fn valid_host(host: &str) -> bool {
    let label = |label: &str| {
        !label.is_empty()
            && label.len() <= 63
            && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };

    host.len() <= 253 && host.contains('.') && host.split('.').all(label)
}

/// A fingerprint, key or secret written as hex, with or without the colons the desktop shows.
pub fn parse_fingerprint(text: &str) -> Option<[u8; 32]> {
    unhex(&text.replace(':', ""))
}

/// Bytes as lowercase hex, as fingerprints, keys and secrets are written between the two sides.
pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

fn unhex<const N: usize>(text: &str) -> Option<[u8; N]> {
    if text.len() != N * 2 || !text.is_ascii() {
        return None;
    }
    let mut bytes = [0u8; N];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }

    Some(bytes)
}

#[cfg(test)]
mod tests;
