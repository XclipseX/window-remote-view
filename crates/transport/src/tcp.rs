//! TCP datagram tunnel: the relay fallback for networks that block UDP. Each datagram is
//! framed as a `u16` big-endian length followed by the payload (same framing the relay's
//! TCP listener speaks; port 21117 by default, or 443 behind restrictive firewalls).

use std::io;
use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

pub const MAX_DATAGRAM: usize = u16::MAX as usize;

/// Datagram-preserving client over one TCP connection. `send` and `recv` take `&self`
/// so they can run from separate tasks (share via `Arc`).
pub struct TcpDatagramTunnel {
    rd: Mutex<OwnedReadHalf>,
    wr: Mutex<OwnedWriteHalf>,
    peer: SocketAddr,
}

impl TcpDatagramTunnel {
    pub async fn connect(addr: SocketAddr) -> io::Result<Self> {
        Self::from_stream(TcpStream::connect(addr).await?)
    }

    pub fn from_stream(s: TcpStream) -> io::Result<Self> {
        s.set_nodelay(true)?; // latency over throughput
        let peer = s.peer_addr()?;
        let (rd, wr) = s.into_split();
        Ok(Self { rd: Mutex::new(rd), wr: Mutex::new(wr), peer })
    }

    pub fn peer_addr(&self) -> SocketAddr {
        self.peer
    }

    /// Send one datagram (frame written atomically with respect to other senders).
    pub async fn send(&self, data: &[u8]) -> io::Result<()> {
        if data.len() > MAX_DATAGRAM {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "datagram larger than 65535 bytes"));
        }
        let mut frame = Vec::with_capacity(2 + data.len());
        frame.extend_from_slice(&(data.len() as u16).to_be_bytes());
        frame.extend_from_slice(data);
        self.wr.lock().await.write_all(&frame).await
    }

    /// Receive the next datagram; `UnexpectedEof` when the peer closed.
    pub async fn recv(&self) -> io::Result<Vec<u8>> {
        let mut rd = self.rd.lock().await;
        let mut len = [0u8; 2];
        rd.read_exact(&mut len).await?;
        let mut buf = vec![0u8; u16::from_be_bytes(len) as usize];
        rd.read_exact(&mut buf).await?;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn frames_roundtrip_and_preserve_boundaries() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        // echo server speaking the same framing, written with raw reads
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut b = [0u8; 4096];
            loop {
                let n = s.read(&mut b).await.unwrap();
                if n == 0 {
                    break;
                }
                s.write_all(&b[..n]).await.unwrap();
            }
        });
        let t = TcpDatagramTunnel::connect(addr).await.unwrap();
        assert_eq!(t.peer_addr(), addr);
        t.send(b"one").await.unwrap();
        t.send(b"").await.unwrap();
        t.send(&[7u8; 1500]).await.unwrap();
        assert_eq!(t.recv().await.unwrap(), b"one");
        assert_eq!(t.recv().await.unwrap(), b"");
        assert_eq!(t.recv().await.unwrap(), vec![7u8; 1500]);
    }

    #[tokio::test]
    async fn rejects_oversize_and_reports_eof() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = l.accept().await; // accept then drop -> EOF
        });
        let t = TcpDatagramTunnel::connect(addr).await.unwrap();
        let e = t.send(&vec![0u8; MAX_DATAGRAM + 1]).await.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(t.recv().await.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }
}
