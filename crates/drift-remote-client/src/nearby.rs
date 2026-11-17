//! Finding computers on this device's networks with Remote Access on. A computer's answer only says where to ask;
//! asking proves who answered (`drift_pairing`).

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use drift_pairing::Announcement;
use tokio::net::UdpSocket;

/// How long answers are collected; a computer on the same network answers within milliseconds.
const LISTEN: Duration = Duration::from_millis(1200);

/// A computer that answered, at the address it answered from.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub address: SocketAddr,
    pub announcement: Announcement,
}

/// Asks every network at `targets` and the system's broadcast address, and lists each computer once, by name.
pub async fn discover(targets: Vec<Ipv4Addr>, port: u16) -> std::io::Result<Vec<Found>> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
    socket.set_broadcast(true)?;
    let mut sent = false;
    for target in std::iter::once(Ipv4Addr::BROADCAST).chain(targets) {
        // An unroutable network fails alone; the others are still asked.
        sent |= socket.send_to(drift_pairing::DISCOVER, (target, port)).await.is_ok();
    }
    if !sent {
        return Err(std::io::Error::other("this device is not on a network"));
    }

    let mut found: Vec<Found> = Vec::new();
    let mut buffer = [0u8; 1024];
    let deadline = tokio::time::Instant::now() + LISTEN;
    while let Ok(received) = tokio::time::timeout_at(deadline, socket.recv_from(&mut buffer)).await {
        let Ok((length, from)) = received else { continue };
        let Ok(announcement) = serde_json::from_slice::<Announcement>(&buffer[..length]) else {
            continue;
        };
        if found.iter().any(|known| known.announcement.id == announcement.id) {
            continue;
        }
        found.push(Found {
            address: SocketAddr::new(from.ip(), announcement.port),
            announcement,
        });
    }
    found.sort_by_key(|found| found.announcement.name.to_lowercase());

    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_computer_answering_on_loopback_is_found_once() {
        let computer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = computer.local_addr().unwrap().port();
        let announcement = Announcement {
            name: "Desk".into(),
            id: "ab".repeat(32),
            port: 41718,
            api: drift_pairing::API_VERSION,
        };
        let reply = serde_json::to_vec(&announcement).unwrap();
        tokio::spawn(async move {
            let mut buffer = [0u8; 64];
            loop {
                let Ok((length, from)) = computer.recv_from(&mut buffer).await else {
                    continue;
                };
                if &buffer[..length] == drift_pairing::DISCOVER {
                    computer.send_to(&reply, from).await.unwrap();
                }
            }
        });

        let found = discover(vec![Ipv4Addr::LOCALHOST, Ipv4Addr::LOCALHOST], port)
            .await
            .unwrap();

        assert_eq!(
            found,
            vec![Found {
                address: "127.0.0.1:41718".parse().unwrap(),
                announcement,
            }],
            "two answers from one computer list it once, at the gateway's port"
        );
    }
}
