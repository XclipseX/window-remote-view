//! tokio `UdpSocket` adapter.
//!
//! [`UdpIo`] sends through a non-blocking `std` socket that shares its file
//! descriptor with the tokio socket used for receiving (tokio's `try_send_to`
//! only works once readiness was observed, which a sync sink cannot await).

use std::io;
use std::net::{SocketAddr, UdpSocket as StdUdpSocket};
use std::sync::{Arc, Mutex};

use tokio::net::UdpSocket;

use crate::DatagramIo;

/// Peer address shared between the receive loop and the sender.
pub type SharedPeer = Arc<Mutex<Option<SocketAddr>>>;

/// Non-blocking datagram sender. Datagrams that would block are dropped (UDP
/// semantics; FEC covers them).
pub struct UdpIo {
    socket: StdUdpSocket,
    peer: SharedPeer,
}

impl UdpIo {
    /// Bind `addr`; returns the sender and the tokio socket to `recv_from` on.
    /// `peer` is fixed for a client; a host passes `None` and stores the source
    /// of each received datagram into [`UdpIo::peer_handle`]. Must be called
    /// inside a tokio runtime.
    pub fn bind(addr: SocketAddr, peer: Option<SocketAddr>) -> io::Result<(Self, UdpSocket)> {
        let std_sock = StdUdpSocket::bind(addr)?;
        std_sock.set_nonblocking(true)?;
        let recv = UdpSocket::from_std(std_sock.try_clone()?)?;
        Ok((
            Self {
                socket: std_sock,
                peer: Arc::new(Mutex::new(peer)),
            },
            recv,
        ))
    }

    pub fn peer_handle(&self) -> SharedPeer {
        self.peer.clone()
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }
}

impl DatagramIo for UdpIo {
    fn send(&mut self, buf: &[u8]) {
        let peer = *self.peer.lock().unwrap();
        if let Some(p) = peer {
            let _ = self.socket.send_to(buf, p);
        }
    }
}
