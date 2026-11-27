use crate::mem::{drain, queue, MemIo, Queue};
use crate::*;
use rv_crypto::Keypair;
use rv_proto::{ControlMessage, InputEvent};
use std::time::{Duration, Instant};

const HOST_ID: &[u8] = b"host-id-0001";

struct Rig {
    host: HostSession<MemIo>,
    client: ClientSession<MemIo>,
    to_host: Queue,
    to_client: Queue,
    /// Synthetic clock fed to the sessions.
    now: Instant,
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
        now: Instant::now(),
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
                if let Err(e) = self.host.handle_datagram_at(&d, self.now) {
                    errs.push(e.to_string());
                }
            }
            for d in b {
                if let Err(e) = self.client.handle_datagram_at(&d, self.now) {
                    errs.push(e.to_string());
                }
            }
        }
    }

    fn advance(&mut self, d: Duration) {
        self.now += d;
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

fn events_h(r: &mut Rig) -> Vec<HostSessionEvent> {
    std::iter::from_fn(|| r.host.poll_session_event()).collect()
}

fn events_c(r: &mut Rig) -> Vec<ClientEvent> {
    std::iter::from_fn(|| r.client.poll_event()).collect()
}

#[test]
fn large_frame_3mb_at_5pct_loss() {
    let mut r = rig("123456", 0.05);
    r.connect();
    r.host.set_loss(0.05);
    for id in 0..3u32 {
        let f = frame_bytes(id, 3 * 1024 * 1024 + id as usize);
        r.host.send_video_frame(id, 1, &f, true).unwrap();
        r.pump();
        let got: Vec<_> = events_c(&mut r)
            .into_iter()
            .filter_map(|e| if let ClientEvent::Frame(f) = e { Some(f) } else { None })
            .collect();
        assert_eq!(got.len(), 1, "frame {id} lost (evicted {})", r.client.evicted_frames());
        assert_eq!(got[0].data, f);
    }
    assert!(r.host.io_mut().dropped > 100);
}

#[test]
fn frame_8mb_roundtrip_lossless() {
    let mut r = rig("123456", 0.0);
    r.connect();
    let f = frame_bytes(1, 8 * 1024 * 1024);
    r.host.send_video_frame(1, 1, &f, true).unwrap();
    r.pump();
    let frame = events_c(&mut r).into_iter().find_map(|e| match e {
        ClientEvent::Frame(f) => Some(f),
        _ => None,
    });
    assert_eq!(frame.unwrap().data, f);
}

#[test]
fn host_times_out_then_accepts_new_handshake() {
    let mut r = rig("123456", 0.0);
    r.connect();
    events_h(&mut r);
    let t0 = r.now;
    r.host.tick(t0 + Duration::from_secs(2));
    assert!(r.host.is_established());
    r.host.tick(t0 + Duration::from_secs(6));
    r.advance(Duration::from_secs(6));
    assert!(!r.host.is_established());
    assert_eq!(events_h(&mut r), vec![HostSessionEvent::Disconnected]);
    assert!(r.host.send_video_frame(1, 0, b"x", false).is_err());

    // Same client restarts (fresh Noise state) -> new session, no process restart.
    let sid_old = r.host.session_id();
    r.client.start().unwrap();
    r.pump();
    assert!(r.host.is_established() && r.client.is_established());
    assert_ne!(r.host.session_id(), sid_old);
    assert!(matches!(events_h(&mut r).as_slice(), [HostSessionEvent::Established { .. }]));
}

#[test]
fn new_handshake_replaces_live_session_only_after_msg3() {
    let mut r = rig("123456", 0.0);
    r.connect();
    events_h(&mut r);
    // A forged msg1 must not disturb the live session.
    let mut rogue = ClientSession::new(
        MemIo::new(r.to_host.clone(), 0.0, 3),
        Keypair::generate().unwrap(),
        "000000",
        HOST_ID,
    )
    .unwrap();
    r.advance(Duration::from_millis(520));
    rogue.start().unwrap();
    r.pump();
    assert!(r.host.is_established());
    assert!(events_h(&mut r).is_empty());
    // Live client still works.
    r.client.send_input(InputEvent::SecureAttention).unwrap();
    r.pump();
    assert!(matches!(events_h(&mut r).as_slice(), [HostSessionEvent::Input { .. }]));
    // The legitimate client reconnecting takes over: Disconnected then Established.
    r.advance(Duration::from_millis(520));
    r.client.start().unwrap();
    r.pump();
    assert!(matches!(
        events_h(&mut r).as_slice(),
        [HostSessionEvent::Disconnected, HostSessionEvent::Established { .. }]
    ));
    assert!(r.host.is_established() && r.client.is_established());
}

#[test]
fn msg1_rate_limited_and_otp_failures_lock_peer() {
    let mut r = rig("123456", 0.0);
    let t0 = Instant::now();
    let m1 = |r: &mut Rig| {
        r.client.start().unwrap();
        drain(&r.to_host).pop().unwrap()
    };
    let d = m1(&mut r);
    r.host.handle_datagram_from(7, &d, t0).unwrap();
    let d = m1(&mut r);
    assert!(matches!(
        r.host.handle_datagram_from(7, &d, t0 + Duration::from_millis(100)),
        Err(SessionError::RateLimited)
    ));
    r.host
        .handle_datagram_from(7, &d, t0 + Duration::from_millis(600))
        .unwrap();

    // Wrong-OTP client from peer 9: 3 failures then locked out.
    let mut bad = rig("654321", 0.0);
    // reuse bad's client against r.host
    let mut now = t0 + Duration::from_secs(2);
    for i in 0..3 {
        bad.client.start().unwrap();
        let m1 = drain(&bad.to_host).pop().unwrap();
        r.host.handle_datagram_from(9, &m1, now).unwrap();
        let m2 = drain(&r.to_client).pop().unwrap();
        bad.client.handle_datagram(&m2).unwrap();
        let m3 = drain(&bad.to_host).pop().unwrap();
        assert!(r.host.handle_datagram_from(9, &m3, now).is_err(), "attempt {i}");
        now += Duration::from_millis(600);
    }
    bad.client.start().unwrap();
    let m1 = drain(&bad.to_host).pop().unwrap();
    assert!(matches!(
        r.host.handle_datagram_from(9, &m1, now),
        Err(SessionError::Locked(_))
    ));
    // Other peers are unaffected.
    r.host.handle_datagram_from(10, &m1, now).unwrap();
}

#[test]
fn client_disconnects_on_silence_and_keepalive_prevents_it() {
    let mut r = rig("123456", 0.0);
    r.connect();
    events_c(&mut r);
    let t0 = r.now;
    // Keepalives from the host keep the client alive.
    r.host.tick(t0 + Duration::from_millis(1500));
    r.pump();
    assert!(events_c(&mut r).is_empty(), "keepalive must not surface as an event");
    r.client.tick(t0 + Duration::from_secs(4));
    assert!(r.client.is_established());
    r.client.tick(t0 + Duration::from_secs(7));
    assert!(!r.client.is_established());
    assert_eq!(events_c(&mut r), vec![ClientEvent::Disconnected]);
    // Reconnect works.
    r.advance(Duration::from_secs(8));
    r.client.start().unwrap();
    r.pump();
    assert!(r.client.is_established());
}

#[test]
fn stream_config_roundtrip_via_session() {
    let mut r = rig("123456", 0.0);
    r.connect();
    events_c(&mut r);
    let cfg = ControlMessage::StreamConfig {
        codec: rv_proto::codec_flags::CODEC_H265,
        width: 2800,
        height: 1752,
        fps: 120,
        bitrate_kbps: 80_000,
    };
    r.host.send_control(cfg.clone()).unwrap();
    r.pump();
    assert_eq!(events_c(&mut r), vec![ClientEvent::Control(cfg)]);
}

#[test]
fn ping_pong_measures_rtt_and_suppresses_events() {
    let mut r = rig("123456", 0.0);
    r.connect();
    events_h(&mut r);
    events_c(&mut r);
    assert_eq!(r.client.rtt_us(), None);

    // Before PING_INTERVAL, tick must not send a Ping.
    r.advance(Duration::from_millis(500));
    r.client.tick(r.now);
    assert!(drain(&r.to_host).is_empty());

    // At PING_INTERVAL (1s), tick sends Ping; host auto-replies Pong with no HostSessionEvent.
    r.advance(Duration::from_millis(500));
    r.client.tick(r.now);
    let pings = drain(&r.to_host);
    assert_eq!(pings.len(), 1, "expected 1 ping datagram");

    // Simulate 5 ms client -> host + 7 ms host -> client = 12_000 us RTT.
    r.advance(Duration::from_millis(5));
    for d in pings {
        r.host.handle_datagram_at(&d, r.now).unwrap();
    }
    assert!(events_h(&mut r).is_empty(), "Ping must not emit HostSessionEvent");
    let pongs = drain(&r.to_client);
    assert_eq!(pongs.len(), 1, "host must auto-reply Pong");

    r.advance(Duration::from_millis(7));
    for d in pongs {
        r.client.handle_datagram_at(&d, r.now).unwrap();
    }
    assert!(events_c(&mut r).is_empty(), "Pong must not emit ClientEvent");
    assert_eq!(r.client.rtt_us(), Some(12_000));

    // Second RTT sample (20_000 us) after another PING_INTERVAL updates EWMA (alpha = 0.25):
    // 0.75 * 12_000 + 0.25 * 20_000 = 14_000.
    r.advance(PING_INTERVAL);
    r.client.tick(r.now);
    let pings = drain(&r.to_host);
    assert_eq!(pings.len(), 1);
    r.advance(Duration::from_millis(10));
    for d in pings {
        r.host.handle_datagram_at(&d, r.now).unwrap();
    }
    let pongs = drain(&r.to_client);
    assert_eq!(pongs.len(), 1);
    r.advance(Duration::from_millis(10));
    for d in pongs {
        r.client.handle_datagram_at(&d, r.now).unwrap();
    }
    assert_eq!(r.client.rtt_us(), Some(14_000));

    // Outlier sample (> 10s) must be ignored.
    r.host
        .send_control(ControlMessage::Pong { ts_us: 0 })
        .unwrap();
    let bogus = drain(&r.to_client);
    // Advance in small steps or just past 10s from epoch while keeping session alive via rx.
    let future = r.now + Duration::from_secs(12);
    for d in bogus {
        r.client.handle_datagram_at(&d, future).unwrap();
    }
    assert_eq!(r.client.rtt_us(), Some(14_000));
    assert!(events_c(&mut r).is_empty());
}

#[test]
fn host_request_log_surfaced_to_client() {
    let mut r = rig("123456", 0.0);
    r.connect();
    events_c(&mut r);
    r.host.send_control(ControlMessage::RequestLog).unwrap();
    r.pump();
    assert_eq!(
        events_c(&mut r),
        vec![ClientEvent::Control(ControlMessage::RequestLog)]
    );
}

#[test]
fn client_loss_permille_tracks_completed_and_evicted_frames() {
    let mut r = rig("123456", 0.0);
    r.connect();
    events_c(&mut r);
    assert_eq!(r.client.loss_permille(), 0);

    // 3 complete frames -> 0 permille loss.
    for id in 0..3u32 {
        let f = frame_bytes(id, 5_000);
        r.host.send_video_frame(id, id * 16_667, &f, id == 0).unwrap();
        r.pump();
    }
    assert_eq!(r.client.loss_permille(), 0);

    // 4th frame: drop 1 shard with FEC disabled (loss = 0.0) so it expires after 250 ms.
    let f = frame_bytes(3, 5_000);
    let sent = r.host.send_video_frame(3, 3 * 16_667, &f, false).unwrap();
    assert!(sent >= 2);
    let mut dgs = drain(&r.to_client);
    dgs.pop(); // drop last shard
    for d in dgs {
        r.client.handle_datagram_at(&d, r.now).unwrap();
    }
    r.advance(Duration::from_millis(300));
    r.client.tick(r.now);
    assert_eq!(r.client.evicted_frames(), 1);
    assert_eq!(r.client.loss_permille(), 250);
}

#[test]
fn start_retransmit_reuses_m1_so_earlier_msg2_completes_handshake() {
    let mut r = rig("123456", 0.0);
    r.client.start().unwrap();
    let m1_first = drain(&r.to_host).pop().unwrap();

    // Host processes the first msg1 and replies with msg2.
    r.host.handle_datagram_at(&m1_first, r.now).unwrap();
    let m2_first = drain(&r.to_client).pop().unwrap();

    // Client retries start() before msg2 arrives: must retransmit exact same m1
    // without overwriting the active Initiator ephemeral keypair.
    r.client.start().unwrap();
    let m1_retry = drain(&r.to_host).pop().unwrap();
    assert_eq!(m1_first, m1_retry, "retransmitted msg1 must match initial msg1");

    // A malformed msg2 with session_id == 0 must not destroy WaitMsg2.
    let bad_sid_msg2 = hs_packet(2, 0, &[1, 2, 3, 4]);
    assert!(matches!(
        r.client.handle_datagram_at(&bad_sid_msg2, r.now),
        Err(SessionError::SessionId)
    ));

    // Now the earlier msg2 arrives and completes the handshake cleanly.
    r.client.handle_datagram_at(&m2_first, r.now).unwrap();
    r.pump();
    assert!(r.client.is_established() && r.host.is_established());
}

#[test]
fn reliable_reset_hint_reconnects_immediately_and_lan_ignores_it() {
    let mut r = rig("123456", 0.0);
    r.host.set_reliable(true);
    r.client.set_reliable(true);
    r.connect();
    events_h(&mut r);
    events_c(&mut r);
    let old_sid = r.host.session_id();

    // Reliable host times out at USB_IDLE_TIMEOUT (2.5s), leaving client thinking it is live.
    r.advance(USB_IDLE_TIMEOUT);
    r.host.tick(r.now);
    assert!(!r.host.is_established());
    assert_eq!(events_h(&mut r), vec![HostSessionEvent::Disconnected]);
    assert!(r.client.is_established());

    // Client sends encrypted data to the now-idle reliable host -> host replies HS_STEP_RESET.
    r.client.send_input(InputEvent::SecureAttention).unwrap();
    let stale_dg = drain(&r.to_host).pop().unwrap();
    assert!(matches!(
        r.host.handle_datagram_at(&stale_dg, r.now),
        Err(SessionError::NotEstablished)
    ));
    let hints = drain(&r.to_client);
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0], hs_packet(HS_STEP_RESET, 0, &[]));
    assert_eq!(hs_parse(&hints[0]).unwrap(), (HS_STEP_RESET, 0, &[][..]));

    // Second stale datagram within 250 ms is throttled (no duplicate reset hint).
    r.advance(Duration::from_millis(100));
    assert!(matches!(
        r.host.handle_datagram_at(&stale_dg, r.now),
        Err(SessionError::NotEstablished)
    ));
    assert!(drain(&r.to_client).is_empty());

    // After 250 ms another stale datagram emits a fresh reset hint.
    r.advance(Duration::from_millis(150));
    assert!(matches!(
        r.host.handle_datagram_at(&stale_dg, r.now),
        Err(SessionError::NotEstablished)
    ));
    assert_eq!(drain(&r.to_client).len(), 1);

    // Reliable client honors HS_STEP_RESET: drops immediately and re-handshakes cleanly.
    r.client.handle_datagram_at(&hints[0], r.now).unwrap();
    assert!(!r.client.is_established());
    assert_eq!(events_c(&mut r), vec![ClientEvent::Disconnected]);

    r.client.start().unwrap();
    r.pump();
    assert!(r.client.is_established() && r.host.is_established());
    assert_ne!(r.host.session_id(), old_sid);

    // Reliable client also honors USB_IDLE_TIMEOUT (2.5s) in tick().
    events_c(&mut r);
    r.advance(USB_IDLE_TIMEOUT);
    r.client.tick(r.now);
    assert!(!r.client.is_established());
    assert_eq!(events_c(&mut r), vec![ClientEvent::Disconnected]);

    // Unreliable (LAN) client ignores HS_STEP_RESET and keeps full 5s IDLE_TIMEOUT;
    // unreliable (LAN) host never emits HS_STEP_RESET when idle.
    let mut lan = rig("123456", 0.0);
    lan.connect();
    events_h(&mut lan);
    events_c(&mut lan);

    let reset = hs_packet(HS_STEP_RESET, 0, &[]);
    lan.client.handle_datagram_at(&reset, lan.now).unwrap();
    assert!(lan.client.is_established());
    assert!(events_c(&mut lan).is_empty());

    // At USB_IDLE_TIMEOUT (2.5s), LAN client and LAN host are still established.
    lan.advance(USB_IDLE_TIMEOUT);
    lan.client.tick(lan.now);
    lan.host.tick(lan.now);
    assert!(lan.client.is_established() && lan.host.is_established());

    // Once LAN host times out after full IDLE_TIMEOUT, stale data must not emit HS_STEP_RESET.
    lan.advance(IDLE_TIMEOUT);
    lan.host.tick(lan.now);
    assert!(!lan.host.is_established());
    drain(&lan.to_client);
    lan.client.send_input(InputEvent::SecureAttention).unwrap();
    let lan_stale = drain(&lan.to_host).pop().unwrap();
    assert!(matches!(
        lan.host.handle_datagram_at(&lan_stale, lan.now),
        Err(SessionError::NotEstablished)
    ));
    assert!(
        drain(&lan.to_client).is_empty(),
        "unreliable host must not send HS_STEP_RESET"
    );
}

#[test]
fn reliable_host_allows_50ms_msg1_retry_while_lan_keeps_500ms() {
    let t0 = Instant::now();

    // Reliable host: 50 ms minimum interval.
    let mut usb = rig("123456", 0.0);
    usb.host.set_reliable(true);
    usb.client.start().unwrap();
    let usb_m1 = drain(&usb.to_host).pop().unwrap();
    usb.host.handle_datagram_from(1, &usb_m1, t0).unwrap();
    assert!(matches!(
        usb.host
            .handle_datagram_from(1, &usb_m1, t0 + Duration::from_millis(25)),
        Err(SessionError::RateLimited)
    ));
    usb.host
        .handle_datagram_from(1, &usb_m1, t0 + USB_HANDSHAKE_MIN_INTERVAL)
        .unwrap();

    // Unreliable (LAN) host: keeps 500 ms minimum interval.
    let mut lan = rig("123456", 0.0);
    lan.client.start().unwrap();
    let lan_m1 = drain(&lan.to_host).pop().unwrap();
    lan.host.handle_datagram_from(1, &lan_m1, t0).unwrap();
    assert!(matches!(
        lan.host
            .handle_datagram_from(1, &lan_m1, t0 + USB_HANDSHAKE_MIN_INTERVAL),
        Err(SessionError::RateLimited)
    ));
    assert!(matches!(
        lan.host
            .handle_datagram_from(1, &lan_m1, t0 + Duration::from_millis(499)),
        Err(SessionError::RateLimited)
    ));
    lan.host
        .handle_datagram_from(1, &lan_m1, t0 + HANDSHAKE_MIN_INTERVAL)
        .unwrap();
}
