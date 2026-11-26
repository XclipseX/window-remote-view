---
type: decision
id: ADR-0002
date: 2026-11-26
status: accepted
---
# ADR-0002: Large frames via protocol v2 + per-block FEC

## Context
v1 `VideoPacket` carried `shard_idx/data_k/parity_m` as u8, so a frame was at most 255 shards (~290 KB incl. parity). A 2800x1752 IDR at high bitrate is several MB.

## Options
1. Widen only the u8 fields to u16 and run one RS code over the whole frame (reed-solomon-simd allows 32768 shards). Simple, but RS decode cost grows with k (thousands of shards), a single lost shard forces a large decode, and recovery latency/CPU spikes exactly on IDRs.
2. **(chosen)** Widen fields to u16 *and* split the frame into FEC blocks of <= 64 data shards each (`block_idx`/`block_count`, u8). Parity is computed per block (`parity_count(k_b, loss)`), recovery is per block (small RS problems, decode as soon as a block has k shards, blocks recover independently), and shards are sent interleaved across blocks so a burst loss is spread out instead of exhausting one block.

## Decision
- `PROTOCOL_VERSION = 2` (outer header high nibble; v1 peers are rejected with `UnsupportedVersion`).
- Video header (`VIDEO_HEADER_LEN = 20`): frame_id u32, capture_ts_us u32, codec_flags u8, block_idx u8, block_count u8, reserved u8, shard_idx u16, data_k u16, parity_m u16, payload_len u16. `shard_idx/data_k/parity_m` are relative to the block.
- Limits: <= 64 data shards per block (`MAX_BLOCK_DATA_SHARDS`), <= 255 blocks (`MAX_BLOCKS`) => ~18.7 MB per frame (8 MB IDR = ~7300 shards = 114 blocks). Larger frames return `FecError::FrameTooLarge` instead of silently dropping parity.
- All shards of a frame have one common even size; the real frame length is the 4-byte prefix the session layer adds.
- Reassembler: a frame is complete when every block has recovered; the 250 ms frame timeout is unchanged.
- Added `ControlMessage::StreamConfig` (host -> client chosen codec/size/fps/bitrate) and `Keepalive` (0x0A, 0x0B).

## Consequences
- ~6 bytes more header per packet (1150 B payload + 20 + 12 outer + 16 tag = 1198 B, still < 1200).
- At 5% loss each 64-shard block gets 10 parity shards, so a 3 MB frame survives random 5% loss (tested in `rv-transport` and `rv-session`).
- Host and client must both be v2 (APK and rv-host.exe rebuilt together).
