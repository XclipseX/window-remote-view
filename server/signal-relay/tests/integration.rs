use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rv_signal_relay::proto::Msg;
use rv_signal_relay::server::{serve, Config, State};
use tokio::net::UdpSocket;
use tokio::time::timeout;

async fn start() -> SocketAddr {
    let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let addr = sock.local_addr().unwrap();
    tokio::spawn(serve(sock, Arc::new(State::new(Config::default()))));
    addr
}

async fn recv(s: &UdpSocket) -> Msg {
    let mut b = [0u8; 2048];
    let (n, _) = timeout(Duration::from_secs(2), s.recv_from(&mut b)).await.expect("timeout").unwrap();
    Msg::decode(&b[..n]).unwrap()
}

#[tokio::test]
async fn register_lookup_relay() {
    let server = start().await;
    let host = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let id = [0xAB; 16];
    let token = [0xCD; 16];

    // register
    host.send_to(&Msg::Register { host_id: id }.encode(), server).await.unwrap();
    assert_eq!(recv(&host).await, Msg::RegisterAck { observed: host.local_addr().unwrap() });

    // lookup: client gets host addr, host is notified of client addr
    client.send_to(&Msg::Lookup { host_id: id }.encode(), server).await.unwrap();
    assert_eq!(recv(&client).await, Msg::LookupResp { host_id: id, addr: Some(host.local_addr().unwrap()) });
    assert_eq!(recv(&host).await, Msg::PeerNotify { client: client.local_addr().unwrap() });

    // unknown host
    client.send_to(&Msg::Lookup { host_id: [1; 16] }.encode(), server).await.unwrap();
    assert_eq!(recv(&client).await, Msg::LookupResp { host_id: [1; 16], addr: None });

    // relay pairing
    host.send_to(&Msg::RelayJoin { token }.encode(), server).await.unwrap();
    assert_eq!(recv(&host).await, Msg::RelayJoinAck { token, paired: false });
    client.send_to(&Msg::RelayJoin { token }.encode(), server).await.unwrap();
    assert_eq!(recv(&client).await, Msg::RelayJoinAck { token, paired: true });
    assert_eq!(recv(&host).await, Msg::RelayJoinAck { token, paired: true });

    // blind forwarding both directions
    let payload = vec![0xDE, 0xAD, 0xBE, 0xEF, 1, 2, 3];
    client.send_to(&Msg::RelayData { token, payload: payload.clone() }.encode(), server).await.unwrap();
    assert_eq!(recv(&host).await, Msg::RelayData { token, payload: payload.clone() });
    host.send_to(&Msg::RelayData { token, payload: vec![42] }.encode(), server).await.unwrap();
    assert_eq!(recv(&client).await, Msg::RelayData { token, payload: vec![42] });

    // probe answered (rv-transport path racing)
    let mut probe = *b"RVP1\x01\0\0\0\0\0\0\0\x07";
    client.send_to(&probe, server).await.unwrap();
    let mut b = [0u8; 64];
    let (n, _) = timeout(Duration::from_secs(2), client.recv_from(&mut b)).await.unwrap().unwrap();
    probe[4] = 2;
    assert_eq!(&b[..n], &probe);
}
