use crate::mem::{drain, queue, MemIo, Queue};
use crate::*;
use rv_crypto::Keypair;
use rv_proto::{ControlMessage, InputEvent};

const HOST_ID: &[u8] = b"host-id-0001";

struct Rig {
    host: HostSession<MemIo>,
    client: ClientSession<MemIo>,
    to_host: Queue,
    to_client: Queue,
}

fn rig(client_otp: &str, loss: f64) -> Rig {
    let (to_host, to_client) = (queue(), queue());
    let host = HostSession::new(
        MemIo::new(to_client.clone(), loss, 7),
        Keypair::generate().unwrap(),
        "123456",
        HOST_ID,
    )
    .unwrap();
    let client = ClientSession::new(
        MemIo::new(to_host.clone(), 0.0, 9),
        Keypair::generate().unwrap(),
        client_otp,
        HOST_ID,
    )
    .unwrap();
    Rig {
        host,
        client,
        to_host,
        to_client,
    }
}

impl Rig {
    /// Deliver everything in flight until quiescent; returns errors seen.
    fn pump(&mut self) -> Vec<String> {
        let mut errs = Vec::new();
        loop {
            let (a, b) = (drain(&self.to_host), drain(&self.to_client));
            if a.is_empty() && b.is_empty() {
                return errs;
            }
            for d in a {
                if let Err(e) = self.host.handle_datagram(&d) {
                    errs.push(e.to_string());
                }
            }
            for d in b {
                if let Err(e) = self.client.handle_datagram(&d) {
                    errs.push(e.to_string());
                }
            }
        }
    }

    fn connect(&mut self) {
        self.client.start().unwrap();
        self.pump();
    }
}

fn frame_bytes(id: u32, n: usize) -> Vec<u8> {
    (0..n)
        .map(|i| (i as u32).wrapping_mul(31).wrapping_add(id) as u8)
        .collect()
}

#[test]
fn handshake_with_otp() {
    let mut r = rig("123456", 0.0);
    r.connect();
    assert!(r.host.is_established() && r.client.is_established());
    assert!(matches!(
        r.host.poll_event(),
        Some(HostEvent::Established { .. })
    ));
    assert!(matches!(
        r.client.poll_event(),
        Some(ClientEvent::Established { .. })
    ));
    assert_ne!(r.host.session_id(), 0);
}

#[test]
fn wrong_otp_fails() {
    let mut r = rig("654321", 0.0);
    r.client.start().unwrap();
    let errs = r.pump();
    assert!(!errs.is_empty(), "host must reject msg3");
    assert!(!r.host.is_established());
    assert!(r.host.poll_event().is_none());
    // Client thinks it is done after msg3, but nothing it sends is accepted.
    r.client.send_input(InputEvent::SecureAttention).ok();
    r.pump();
    assert!(r.host.poll_event().is_none());
    assert!(r.host.send_video_frame(1, 0, b"x", false).is_err());
}

#[test]
fn data_before_handshake_rejected() {
    let mut r = rig("123456", 0.0);
    assert!(r.client.send_input(InputEvent::SecureAttention).is_err());
    assert!(r.host.send_video_frame(1, 0, b"x", false).is_err());
}

#[test]
fn video_with_loss_all_frames_reconstructed() {
    let mut r = rig("123456", 0.05);
    r.connect();
    r.host.set_loss(0.05);
    let n = 30;
    let mut want = Vec::new();
    for id in 0..n {
        let f = frame_bytes(id, 20_000 + id as usize * 997);
        r.host
            .send_video_frame(id, id * 16_667, &f, id % 10 == 0)
            .unwrap();
        want.push(f);
        r.pump();
    }
    let mut got = Vec::new();
    while let Some(e) = r.client.poll_event() {
        if let ClientEvent::Frame(f) = e {
            got.push(f);
        }
    }
    assert!(
        r.host.io_mut().dropped > 0,
        "test must actually lose shards"
    );
    assert_eq!(
        got.len(),
        n as usize,
        "evicted {}",
        r.client.evicted_frames()
    );
    for f in got {
        assert_eq!(f.data, want[f.frame_id as usize]);
        assert_eq!(f.capture_ts_us, f.frame_id * 16_667);
        assert_eq!(f.keyframe, f.frame_id % 10 == 0);
        assert_eq!(f.codec, rv_proto::codec_flags::CODEC_H265);
    }
}

#[test]
fn input_and_control_roundtrip() {
    let mut r = rig("123456", 0.0);
    r.connect();
    r.host.poll_event();
    r.client.poll_event();

    let ev = InputEvent::Key {
        scancode: 0x1E,
        extended: false,
        down: true,
    };
    r.client.send_input(ev.clone()).unwrap();
    r.client
        .send_input(InputEvent::Text("héllo".into()))
        .unwrap();
    r.client.request_idr().unwrap();
    let q = ControlMessage::QualitySliderUpdate {
        quality: 80,
        width: 1920,
        height: 1080,
        fps: 60,
    };
    r.client.send_control(q.clone()).unwrap();
    let fb = ControlMessage::FeedbackStats {
        rtt_us: 4000,
        loss_permille: 50,
        recv_kbps: 15000,
        decode_us: 2000,
    };
    r.client.send_control(fb.clone()).unwrap();
    r.pump();

    assert_eq!(
        r.host.poll_event(),
        Some(HostEvent::Input { seq: 0, event: ev })
    );
    assert_eq!(
        r.host.poll_event(),
        Some(HostEvent::Input {
            seq: 1,
            event: InputEvent::Text("héllo".into())
        })
    );
    assert_eq!(
        r.host.poll_event(),
        Some(HostEvent::Control(ControlMessage::RequestIdr))
    );
    assert_eq!(r.host.poll_event(), Some(HostEvent::Control(q)));
    assert_eq!(r.host.poll_event(), Some(HostEvent::Control(fb)));
    assert!(r.host.poll_event().is_none());

    // host -> client control
    let upd = ControlMessage::DisplayConfigChange {
        w: 2560,
        h: 1440,
        fps: 120,
    };
    r.host.send_control(upd.clone()).unwrap();
    r.pump();
    assert_eq!(r.client.poll_event(), Some(ClientEvent::Control(upd)));
}

#[test]
fn replay_and_tamper_rejected() {
    let mut r = rig("123456", 0.0);
    r.connect();
    r.client.send_input(InputEvent::SecureAttention).unwrap();
    let dg = drain(&r.to_host).pop().unwrap();
    r.host.handle_datagram(&dg).unwrap();
    assert!(matches!(
        r.host.poll_event(),
        Some(HostEvent::Established { .. })
    ));
    assert!(matches!(r.host.poll_event(), Some(HostEvent::Input { .. })));
    // exact replay
    assert!(r.host.handle_datagram(&dg).is_err());
    assert!(r.host.poll_event().is_none());
    // header tamper (flags nibble changes AAD, nonce unchanged)
    r.client.send_input(InputEvent::SecureAttention).unwrap();
    let mut dg2 = drain(&r.to_host).pop().unwrap();
    dg2[0] ^= 0x01;
    assert!(r.host.handle_datagram(&dg2).is_err());
    // ciphertext tamper
    r.client.send_input(InputEvent::SecureAttention).unwrap();
    let mut dg3 = drain(&r.to_host).pop().unwrap();
    let last = dg3.len() - 1;
    dg3[last] ^= 0xFF;
    assert!(r.host.handle_datagram(&dg3).is_err());
    assert!(r.host.poll_event().is_none());
}

#[test]
fn feedback_adjusts_fec() {
    let mut r = rig("123456", 0.0);
    r.connect();
    let f = frame_bytes(1, 10_000);
    let n0 = r.host.send_video_frame(1, 0, &f, false).unwrap();
    r.client
        .send_control(ControlMessage::FeedbackStats {
            rtt_us: 1,
            loss_permille: 100,
            recv_kbps: 1,
            decode_us: 1,
        })
        .unwrap();
    r.pump();
    let n1 = r.host.send_video_frame(2, 0, &f, false).unwrap();
    assert!(n1 > n0);
}

#[tokio::test]
async fn localhost_udp() {
    use std::time::Duration;
    use tokio::time::timeout;

    let (host_io, hs) = udp::UdpIo::bind("127.0.0.1:0".parse().unwrap(), None).unwrap();
    let peer = host_io.peer_handle();
    let mut host =
        HostSession::new(host_io, Keypair::generate().unwrap(), "246810", HOST_ID).unwrap();
    let (client_io, cs) = udp::UdpIo::bind(
        "127.0.0.1:0".parse().unwrap(),
        Some(hs.local_addr().unwrap()),
    )
    .unwrap();
    let mut client =
        ClientSession::new(client_io, Keypair::generate().unwrap(), "246810", HOST_ID).unwrap();

    client.start().unwrap();
    let mut hbuf = vec![0u8; 2048];
    let mut cbuf = vec![0u8; 2048];
    // msg1 -> host, msg2 -> client (which sends msg3), msg3 -> host
    for _ in 0..2 {
        let (n, from) = timeout(Duration::from_secs(5), hs.recv_from(&mut hbuf))
            .await
            .unwrap()
            .unwrap();
        *peer.lock().unwrap() = Some(from);
        host.handle_datagram(&hbuf[..n]).unwrap();
        if !client.is_established() {
            let n = timeout(Duration::from_secs(5), cs.recv(&mut cbuf))
                .await
                .unwrap()
                .unwrap();
            client.handle_datagram(&cbuf[..n]).unwrap();
        }
    }
    assert!(host.is_established() && client.is_established());

    let frame = frame_bytes(42, 30_000);
    let sent = host.send_video_frame(42, 123, &frame, true).unwrap();
    let mut got = None;
    for _ in 0..sent {
        let n = timeout(Duration::from_secs(5), cs.recv(&mut cbuf))
            .await
            .unwrap()
            .unwrap();
        client.handle_datagram(&cbuf[..n]).unwrap();
    }
    while let Some(e) = client.poll_event() {
        if let ClientEvent::Frame(f) = e {
            got = Some(f);
        }
    }
    let f = got.expect("frame");
    assert_eq!(f.data, frame);
    assert!(f.keyframe);

    client
        .send_input(InputEvent::MouseAbs { x: 1, y: 2 })
        .unwrap();
    let (n, _) = timeout(Duration::from_secs(5), hs.recv_from(&mut hbuf))
        .await
        .unwrap()
        .unwrap();
    host.handle_datagram(&hbuf[..n]).unwrap();
    let mut saw = false;
    while let Some(e) = host.poll_event() {
        saw |= matches!(
            e,
            HostEvent::Input {
                event: InputEvent::MouseAbs { x: 1, y: 2 },
                ..
            }
        );
    }
    assert!(saw);
}
