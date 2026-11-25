//! In-memory loopback: handshake, then stream synthetic frames with FEC and
//! 5% shard loss, printing throughput.
use std::time::Instant;

use rv_crypto::Keypair;
use rv_session::mem::{drain, queue, MemIo};
use rv_session::*;

fn main() {
    let (to_host, to_client) = (queue(), queue());
    let host_id = b"example-host-id";
    let mut host = HostSession::new(
        MemIo::new(to_client.clone(), 0.05, 1),
        Keypair::generate().unwrap(),
        "123456",
        host_id,
    )
    .unwrap();
    let mut client = ClientSession::new(
        MemIo::new(to_host.clone(), 0.0, 2),
        Keypair::generate().unwrap(),
        "123456",
        host_id,
    )
    .unwrap();
    host.set_loss(0.10);

    client.start().unwrap();
    while !(host.is_established() && client.is_established()) {
        for d in drain(&to_host) {
            host.handle_datagram(&d).unwrap();
        }
        for d in drain(&to_client) {
            client.handle_datagram(&d).unwrap();
        }
    }
    println!("handshake ok, session_id={:08x}", host.session_id());

    let (w, h, fps) = (1920, 1080, 60);
    let bps = slider_to_bitrate(DEFAULT_QUALITY, w, h, fps, DEFAULT_CODEC);
    let frame_len = (bps / 8 / u64::from(fps)) as usize;
    println!(
        "slider {DEFAULT_QUALITY} -> {} Mbps, frame ~{} bytes",
        bps as f64 / 1e6,
        frame_len
    );

    let n = 600u32;
    let (mut got, mut bytes) = (0u32, 0u64);
    let t = Instant::now();
    for id in 0..n {
        let data: Vec<u8> = (0..frame_len).map(|i| (i as u32 ^ id) as u8).collect();
        host.send_video_frame(id, id.wrapping_mul(16_667), &data, id % 120 == 0)
            .unwrap();
        for d in drain(&to_client) {
            client.handle_datagram(&d).unwrap();
        }
        while let Some(e) = client.poll_event() {
            if let ClientEvent::Frame(f) = e {
                assert_eq!(f.data, data);
                got += 1;
                bytes += f.data.len() as u64;
            }
        }
    }
    let s = t.elapsed().as_secs_f64();
    let io = host.io_mut();
    println!(
        "{got}/{n} frames, {} datagrams ({} dropped), {:.1} MB in {:.3}s = {:.0} Mbit/s, {:.0} fps",
        io.sent,
        io.dropped,
        bytes as f64 / 1e6,
        s,
        bytes as f64 * 8.0 / s / 1e6,
        f64::from(got) / s
    );
}
