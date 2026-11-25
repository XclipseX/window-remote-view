use super::*;

fn rt<T, E, D>(v: &T, enc: E, dec: D)
where
    T: PartialEq + core::fmt::Debug,
    E: Fn(&T, &mut Vec<u8>) -> Result<()>,
    D: Fn(&[u8]) -> Result<(T, usize)>,
{
    let mut buf = vec![0xAA]; // pre-existing content must be preserved
    enc(v, &mut buf).unwrap();
    let body = &buf[1..];
    let (got, n) = dec(body).unwrap();
    assert_eq!(&got, v);
    assert_eq!(n, body.len());
    // every strict prefix must fail cleanly
    for cut in 0..body.len() {
        assert!(dec(&body[..cut]).is_err(), "prefix {cut} decoded");
    }
    // trailing bytes are not consumed
    let mut extra = body.to_vec();
    extra.extend_from_slice(&[1, 2, 3]);
    assert_eq!(dec(&extra).unwrap().1, body.len());
}

#[test]
fn outer_roundtrip_and_layout() {
    let h = OuterHeader {
        flags: 0x5,
        session_id: 0xDEADBEEF,
        packet_type: PacketType::Control,
        nonce_counter: 0x0102_0304_0506,
    };
    let mut b = Vec::new();
    h.encode(&mut b).unwrap();
    assert_eq!(
        b,
        [0x15, 0xDE, 0xAD, 0xBE, 0xEF, 0x04, 1, 2, 3, 4, 5, 6]
    );
    rt(&h, OuterHeader::encode, OuterHeader::decode);
}

#[test]
fn outer_rejections() {
    let mut b = Vec::new();
    let mut h = OuterHeader::new(1, PacketType::Video, NONCE_MAX + 1);
    assert!(h.encode(&mut b).is_err());
    assert!(b.is_empty());
    h.nonce_counter = NONCE_MAX;
    h.flags = 16;
    assert!(h.encode(&mut b).is_err());
    h.flags = 0;
    h.encode(&mut b).unwrap();
    let mut bad = b.clone();
    bad[0] = 0x20;
    assert_eq!(
        OuterHeader::decode(&bad),
        Err(ProtoError::UnsupportedVersion(2))
    );
    bad[0] = 0x10;
    bad[5] = 0x7F;
    assert_eq!(
        OuterHeader::decode(&bad),
        Err(ProtoError::UnknownPacketType(0x7F))
    );
}

fn video(len: usize) -> VideoPacket {
    VideoPacket {
        frame_id: 7,
        capture_ts_us: 123456,
        codec_flags: codec_flags::CODEC_H264 | codec_flags::FLAG_KEYFRAME,
        shard_idx: 3,
        data_k: 4,
        parity_m: 2,
        payload: (0..len).map(|i| i as u8).collect(),
    }
}

#[test]
fn video_roundtrip() {
    rt(&video(0), VideoPacket::encode, VideoPacket::decode);
    rt(&video(1150), VideoPacket::encode, VideoPacket::decode);
    let mut b = Vec::new();
    video(10).encode(&mut b).unwrap();
    assert_eq!(b.len(), VIDEO_HEADER_LEN + 10);
    assert_eq!(&b[12..14], &[0, 10]);
}

#[test]
fn video_rejections() {
    let mut b = Vec::new();
    assert!(matches!(
        video(MAX_SHARD_PAYLOAD + 1).encode(&mut b),
        Err(ProtoError::TooLarge { .. })
    ));
    assert!(b.is_empty());
    let mut p = video(1);
    p.shard_idx = 6;
    assert!(p.encode(&mut b).is_err());
    p.shard_idx = 0;
    p.data_k = 0;
    assert!(p.encode(&mut b).is_err());
    // oversize on decode
    video(1150).encode(&mut b).unwrap();
    b[12..14].copy_from_slice(&1151u16.to_be_bytes());
    b.push(0);
    assert!(matches!(
        VideoPacket::decode(&b),
        Err(ProtoError::TooLarge { .. })
    ));
}

#[test]
fn audio_roundtrip_and_limits() {
    let a = AudioPacket {
        seq_num: 65535,
        capture_ts_us: 99,
        duration_ms: 10,
        channels_rate: 0x21,
        payload: vec![9; 120],
    };
    rt(&a, AudioPacket::encode, AudioPacket::decode);
    let big = AudioPacket {
        payload: vec![0; MAX_AUDIO_PAYLOAD + 1],
        ..a
    };
    assert!(big.encode(&mut Vec::new()).is_err());
}

fn all_input_events() -> Vec<InputEvent> {
    vec![
        InputEvent::MouseRel { dx: -5, dy: 32767 },
        InputEvent::MouseAbs { x: 0, y: 65535 },
        InputEvent::MouseButton { button: 2, down: true },
        InputEvent::Wheel { dx: -120, dy: 120 },
        InputEvent::Key { scancode: 0xE05B, extended: true, down: false },
        InputEvent::Key { scancode: 0x1E, extended: false, down: true },
        InputEvent::Text("Xin chào, thế giới ạ".into()),
        InputEvent::Text(String::new()),
        InputEvent::Touch { points: vec![] },
        InputEvent::Touch {
            points: (0..10)
                .map(|i| TouchPoint { id: i, phase: phase::MOVE, x: i as u16 * 100, y: 5, pressure: 77 })
                .collect(),
        },
        InputEvent::Pen { x: 1, y: 2, pressure: 3, tilt_x: -60, tilt_y: 60, buttons: 3, phase: phase::DOWN },
        InputEvent::SecureAttention,
    ]
}

#[test]
fn input_roundtrip() {
    for (i, event) in all_input_events().into_iter().enumerate() {
        let p = InputPacket { input_seq: i as u16, flags: 0x80, event };
        rt(&p, InputPacket::encode, InputPacket::decode);
    }
}

#[test]
fn input_rejections() {
    let mut b = Vec::new();
    let too_many = InputEvent::Touch {
        points: vec![TouchPoint { id: 0, phase: 0, x: 0, y: 0, pressure: 0 }; 11],
    };
    assert!(InputPacket { input_seq: 0, flags: 0, event: too_many }.encode(&mut b).is_err());
    let long = InputEvent::Text("a".repeat(MAX_TEXT_BYTES + 1));
    assert!(InputPacket { input_seq: 0, flags: 0, event: long }.encode(&mut b).is_err());
    assert!(b.is_empty());
    // touch count 11 on decode
    assert!(InputEvent::decode_payload(sub_type::TOUCH, &[11]).is_err());
    // bad utf-8
    assert_eq!(
        InputEvent::decode_payload(sub_type::TEXT, &[0xFF, 0xFE]),
        Err(ProtoError::InvalidUtf8)
    );
    // unknown subtype, bad bool, trailing bytes, bad key flags
    assert_eq!(InputEvent::decode_payload(0xEE, &[]), Err(ProtoError::UnknownSubType(0xEE)));
    assert!(InputEvent::decode_payload(sub_type::MOUSE_BUTTON, &[1, 2]).is_err());
    assert!(InputEvent::decode_payload(sub_type::SECURE_ATTENTION, &[0]).is_err());
    assert!(InputEvent::decode_payload(sub_type::KEY, &[0, 1, 4]).is_err());
    // payload_len lying about length
    let mut ok = Vec::new();
    InputPacket { input_seq: 1, flags: 0, event: InputEvent::MouseAbs { x: 1, y: 2 } }
        .encode(&mut ok)
        .unwrap();
    ok[5] = 3;
    assert!(InputPacket::decode(&ok).is_err());
}

fn all_controls() -> Vec<ControlMessage> {
    vec![
        ControlMessage::Hello {
            proto_version: 1,
            codecs: codec_mask::H264 | codec_mask::AV1,
            max_w: 3840,
            max_h: 2160,
            max_fps: 144,
        },
        ControlMessage::DisplayConfigChange { w: 1920, h: 1080, fps: 60 },
        ControlMessage::RequestIdr,
        ControlMessage::CursorShape { w: 2, h: 3, hot_x: 1, hot_y: 1, rgba: vec![7; 24] },
        ControlMessage::CursorShape { w: 0, h: 0, hot_x: 0, hot_y: 0, rgba: vec![] },
        ControlMessage::ClipboardSync("clipboard \u{1F600} text".into()),
        ControlMessage::QualitySliderUpdate { quality: 100, width: 1280, height: 720, fps: 30 },
        ControlMessage::QualitySliderUpdate { quality: 1, width: 0, height: 0, fps: 0 },
        ControlMessage::PauseVideo(true),
        ControlMessage::PauseVideo(false),
        ControlMessage::FeedbackStats { rtt_us: 12_000, loss_permille: 25, recv_kbps: 20_000, decode_us: 3_500 },
        ControlMessage::Ack { ctrl_seq: 42 },
    ]
}

#[test]
fn control_roundtrip() {
    for (i, message) in all_controls().into_iter().enumerate() {
        let p = ControlPacket { ctrl_seq: i as u16, message };
        rt(&p, ControlPacket::encode, ControlPacket::decode);
    }
}

#[test]
fn control_rejections() {
    let enc = |m: ControlMessage| ControlPacket { ctrl_seq: 0, message: m }.encode(&mut Vec::new());
    assert!(enc(ControlMessage::QualitySliderUpdate { quality: 0, width: 1, height: 1, fps: 1 }).is_err());
    assert!(enc(ControlMessage::QualitySliderUpdate { quality: 101, width: 1, height: 1, fps: 1 }).is_err());
    // rgba size mismatch
    assert!(enc(ControlMessage::CursorShape { w: 2, h: 2, hot_x: 0, hot_y: 0, rgba: vec![0; 15] }).is_err());
    // cursor too big for u16 payload (128*128*4 = 65536)
    assert!(enc(ControlMessage::CursorShape { w: 128, h: 128, hot_x: 0, hot_y: 0, rgba: vec![0; 65536] }).is_err());
    // clipboard too big
    assert!(enc(ControlMessage::ClipboardSync("a".repeat(65536))).is_err());
    assert!(enc(ControlMessage::ClipboardSync("a".repeat(65535))).is_ok());
    // decode side
    assert!(ControlMessage::decode_payload(op_code::QUALITY_SLIDER_UPDATE, &[0, 0, 1, 0, 1, 0, 1]).is_err());
    assert_eq!(ControlMessage::decode_payload(0xEE, &[]), Err(ProtoError::UnknownOpCode(0xEE)));
    assert!(ControlMessage::decode_payload(op_code::PAUSE_VIDEO, &[2]).is_err());
    assert!(ControlMessage::decode_payload(op_code::REQUEST_IDR, &[0]).is_err());
    assert_eq!(
        ControlMessage::decode_payload(op_code::CLIPBOARD_SYNC, &[0xC0]),
        Err(ProtoError::InvalidUtf8)
    );
    // cursor header claims huge image with tiny body: must not allocate/panic
    let hdr = [0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0];
    assert!(ControlMessage::decode_payload(op_code::CURSOR_SHAPE, &hdr).is_err());
}

#[test]
fn packet_enum_roundtrip() {
    let pkts = vec![
        Packet::Video(video(100)),
        Packet::Audio(AudioPacket { seq_num: 1, capture_ts_us: 2, duration_ms: 5, channels_rate: 3, payload: vec![1, 2, 3] }),
        Packet::Input(InputPacket { input_seq: 9, flags: 0, event: InputEvent::SecureAttention }),
        Packet::Control(ControlPacket { ctrl_seq: 3, message: ControlMessage::RequestIdr }),
    ];
    for p in pkts {
        let mut b = Vec::new();
        p.encode(&mut b).unwrap();
        let (d, n) = Packet::decode(p.packet_type(), &b).unwrap();
        assert_eq!(d, p);
        assert_eq!(n, b.len());
        // wrong type must not panic
        for t in [PacketType::Video, PacketType::Audio, PacketType::Input, PacketType::Control] {
            let _ = Packet::decode(t, &b);
        }
    }
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
}

#[test]
fn fuzz_random_bytes_never_panic() {
    let mut rng = Lcg(0x1234_5678);
    let types = [PacketType::Video, PacketType::Audio, PacketType::Input, PacketType::Control];
    for _ in 0..50_000 {
        let len = (rng.next() % 80) as usize;
        let mut data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        // bias toward plausible headers sometimes
        if len > 5 && rng.next() & 1 == 0 {
            data[4] = 0;
            data[5] = (rng.next() % 4) as u8;
        }
        let _ = OuterHeader::decode(&data);
        for t in types {
            let _ = Packet::decode(t, &data);
        }
        for sub in 0..=12u8 {
            let _ = InputEvent::decode_payload(sub, &data);
            let _ = ControlMessage::decode_payload(sub, &data);
        }
    }
}

#[test]
fn fuzz_mutated_valid_packets() {
    let mut rng = Lcg(99);
    let mut samples = Vec::new();
    for e in all_input_events() {
        let mut b = Vec::new();
        InputPacket { input_seq: 1, flags: 0, event: e }.encode(&mut b).unwrap();
        samples.push((PacketType::Input, b));
    }
    for m in all_controls() {
        let mut b = Vec::new();
        ControlPacket { ctrl_seq: 1, message: m }.encode(&mut b).unwrap();
        samples.push((PacketType::Control, b));
    }
    let mut b = Vec::new();
    video(200).encode(&mut b).unwrap();
    samples.push((PacketType::Video, b));
    for (t, base) in &samples {
        for _ in 0..2000 {
            let mut d = base.clone();
            for _ in 0..(1 + rng.next() % 3) {
                let i = rng.next() as usize % d.len();
                d[i] = rng.next() as u8;
            }
            let _ = Packet::decode(*t, &d);
        }
    }
}
