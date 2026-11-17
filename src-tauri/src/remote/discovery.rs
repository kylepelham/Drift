//! Answers devices on this network looking for computers to connect to, while Remote Access is on. The answer only
//! names this PC; a device trusts nothing in it, as linking proves who it reached (`drift_pairing`).

use std::net::Ipv4Addr;

use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::HTTP_PORT;

/// Starts answering on the gateway's port over UDP. Another program holding that port only keeps this PC off the
/// nearby list: a device can still connect by its address, which Remote Access settings show.
pub(super) async fn start(
    announcement: drift_pairing::Announcement,
    shutdown: watch::Receiver<bool>,
) -> Option<JoinHandle<()>> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, HTTP_PORT)).await.ok()?;
    let reply = serde_json::to_vec(&announcement).ok()?;

    Some(tokio::spawn(answer(socket, reply, shutdown)))
}

async fn answer(socket: UdpSocket, reply: Vec<u8>, mut shutdown: watch::Receiver<bool>) {
    let mut buffer = [0u8; 64];
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            received = socket.recv_from(&mut buffer) => {
                // Windows reports a reset from an earlier reply's unreachable peer as a failed receive; keep going.
                let Ok((length, peer)) = received else { continue };
                if &buffer[..length] == drift_pairing::DISCOVER && drift_pairing::on_local_network(peer.ip()) {
                    let _ = socket.send_to(&reply, peer).await;
                }
            }
        }
    }
}
