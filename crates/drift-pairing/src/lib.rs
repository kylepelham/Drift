//! What a Drift PC and the Drift app agree on to pair: the QR payload and the remote API version. Neither side's
//! engine or UI is in here.

use std::fmt::Write as _;
use std::net::IpAddr;

/// The remote API the PC's gateway serves; the app refuses a PC whose major version differs.
pub const API_VERSION: u32 = 1;

const SCHEME: &str = "drift";
const PAIR: &str = "pair";
const PAYLOAD_VERSION: &str = "1";

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum PairingError {
    #[error("not a Drift pairing code")]
    NotPairing,
    #[error("this pairing code needs a newer Drift app")]
    Version,
    #[error("the pairing code has no {0}")]
    Missing(&'static str),
    #[error("the pairing code's {0} is not valid")]
    Invalid(&'static str),
}

/// Where the PC answers over the internet, through OpenTunnel, and the key its certificate must carry.
#[derive(Clone, Debug, PartialEq)]
pub struct Tunnel {
    /// The PC's own name under the tunnel, such as `k2v7.abcd1234efgh.opentunnel.xyz`.
    pub host: String,
    /// SHA-256 of the certificate's public key. The relay could have a certificate issued for the host, but not
    /// one for this key, which never leaves the PC.
    pub key: [u8; 32],
}

/// Everything the desktop's pairing QR code carries.
#[derive(Clone, Debug, PartialEq)]
pub struct Pairing {
    /// The computer's name, to label it in the app.
    pub name: String,
    /// SHA-256 of the PC's Remote Access certificate authority: the only certificate the app trusts for it.
    pub fingerprint: [u8; 32],
    /// Every address the PC can be reached at on a network (its LAN, Tailscale), most likely first.
    pub addresses: Vec<IpAddr>,
    pub port: u16,
    /// The LAN adapter's hardware address, for Wake-on-LAN.
    pub mac: Option<[u8; 6]>,
    /// The PC through OpenTunnel, when it can be reached from anywhere.
    pub tunnel: Option<Tunnel>,
    /// The one-time ticket that signs the app in.
    pub ticket: String,
}

impl Pairing {
    pub fn to_uri(&self) -> String {
        let addresses: Vec<String> = self.addresses.iter().map(ToString::to_string).collect();
        let mut uri = url::Url::parse(&format!("{SCHEME}://{PAIR}")).expect("a fixed, valid URI");
        {
            let mut query = uri.query_pairs_mut();
            query
                .append_pair("v", PAYLOAD_VERSION)
                .append_pair("n", &self.name)
                .append_pair("f", &hex(&self.fingerprint))
                .append_pair("a", &addresses.join(","))
                .append_pair("p", &self.port.to_string());
            if let Some(mac) = &self.mac {
                query.append_pair("m", &hex(mac));
            }
            if let Some(tunnel) = &self.tunnel {
                query.append_pair("h", &tunnel.host).append_pair("k", &hex(&tunnel.key));
            }
            query.append_pair("t", &self.ticket);
        }

        uri.into()
    }

    pub fn parse(text: &str) -> Result<Self, PairingError> {
        let uri = url::Url::parse(text.trim()).map_err(|_| PairingError::NotPairing)?;
        if uri.scheme() != SCHEME || uri.host_str() != Some(PAIR) {
            return Err(PairingError::NotPairing);
        }
        let field = |key: &'static str| {
            uri.query_pairs()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.into_owned())
        };
        if field("v").as_deref() != Some(PAYLOAD_VERSION) {
            return Err(PairingError::Version);
        }

        let required = |key: &'static str, what: &'static str| field(key).ok_or(PairingError::Missing(what));
        let addresses = addresses(&field("a").unwrap_or_default())?;
        let tunnel = tunnel(field("h"), field("k"))?;
        let ticket = required("t", "ticket")?;
        if (addresses.is_empty() && tunnel.is_none()) || ticket.is_empty() {
            return Err(PairingError::Missing("address or ticket"));
        }

        Ok(Self {
            name: field("n").unwrap_or_default(),
            fingerprint: unhex(&required("f", "fingerprint")?).ok_or(PairingError::Invalid("fingerprint"))?,
            addresses,
            port: required("p", "port")?
                .parse()
                .map_err(|_| PairingError::Invalid("port"))?,
            mac: field("m")
                .map(|mac| unhex(&mac).ok_or(PairingError::Invalid("hardware address")))
                .transpose()?,
            tunnel,
            ticket,
        })
    }
}

fn addresses(list: &str) -> Result<Vec<IpAddr>, PairingError> {
    list.split(',')
        .filter(|address| !address.is_empty())
        .map(|address| address.parse().map_err(|_| PairingError::Invalid("address")))
        .collect()
}

/// The tunnel needs both its host and its key; either alone is a damaged code.
fn tunnel(host: Option<String>, key: Option<String>) -> Result<Option<Tunnel>, PairingError> {
    match (host, key) {
        (None, None) => Ok(None),
        (Some(host), Some(key)) if valid_host(&host) => Ok(Some(Tunnel {
            host,
            key: unhex(&key).ok_or(PairingError::Invalid("tunnel key"))?,
        })),
        _ => Err(PairingError::Invalid("tunnel")),
    }
}

/// A DNS name of plain labels, so a code cannot point the app at a URL with a path, port or credentials.
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

/// A fingerprint written as hex, with or without the colons the desktop shows.
pub fn parse_fingerprint(text: &str) -> Option<[u8; 32]> {
    unhex(&text.replace(':', ""))
}

/// Bytes as lowercase hex, as fingerprints, keys and hardware addresses are written in the pairing code.
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
