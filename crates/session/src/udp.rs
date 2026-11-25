//! tokio `UdpSocket` adapter.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::net::UdpSocket;

use crate::DatagramIo;

/// Peer address shared between the receive loop and the sender.
pub type SharedPeer = Arc<Mutex<Option<SocketAddr>>>;

/// Non-blocking sender over a shared tokio socket. Datagrams that would block
/// are dropped (UDP semantics; FEC covers them).
pub struct UdpIo {
    socket: Arc<UdpSocket>,
    peer: SharedPeer,
}

impl UdpIo {
    /// `peer` is fixed for a client; a host passes `None` and updates the
    /// returned [`SharedPeer`] with the source of each received datagram.
    pub fn new(socket: Arc<UdpSocket>, peer: Option<SocketAddr>) -> Self {
        Self { socket, peer: Arc::new(Mutex::new(peer)) }
    }

    pub fn peer_handle(&self) -> SharedPeer {
        self.peer.clone()
    }
}

impl DatagramIo for UdpIo {
    fn send(&mut self, buf: &[u8]) {
        let peer = *self.peer.lock().unwrap();
        if let Some(p) = peer {
            let _ = self.socket.try_send_to(buf, p);
        }
    }
}
