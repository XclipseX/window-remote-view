---
type: decision
id: ADR-0005
date: 2026-11-27
status: accepted
tags: [adr, host-win, amf, openh264, telemetry, lan, security]
---
# ADR-0005: Native AMD AMF DX11 Encoder, Multi-Slice OpenH264, Telemetry RTT/Decode & LAN End-to-End

## Context
- On ASUS Vivobook S14 OLED (Radeon 890M, VCN 4.0.5), Media Foundation async MFT (`AMDh265Encoder`) is fragile and adds a heavy COM/MF event-pump wrapper over AMD's own AMF runtime (`amfrt64.dll`), while high-level `openh264` 0.9.10 forces `SM_SINGLE_SLICE` (1 CPU thread).
- Neither host nor Android measured real RTT (`Ping`/`Pong`), MediaCodec `decode_us`, or `FeedbackStats`; Android `nativeConnect` (LAN) and `nativeStartDiscovery` were stubs; Windows "Xuất log" could not pull Android logs automatically; updaters lacked SHA-256 verification.

## Decision
1. **Native AMD AMF (`crates/host-win/src/encode/amf_ffi.rs`, `amf.rs`):** Zero-dependency `#[repr(C)]` FFI loading `C:\Windows\System32\amfrt64.dll` via `LOAD_LIBRARY_SEARCH_SYSTEM32` (`AMFQueryVersion` + `AMFInit`), sharing the DXGI capture `ID3D11Device`, converting/scaling via validated D3D11 VideoProcessor to a 4-texture NV12 ring (fallback BGRA), setting `Usage = ULTRA_LOW_LATENCY` first, `QueryTimeout = 50ms`, and `HeaderInsertionMode = IDR_ALIGNED`. Encoder priority: `AmfHevc -> AmfH264 -> MfHevcHw (D3D) -> MfH264Hw (D3D) -> MfHevcHw (SysMem) -> MfH264Hw (SysMem) -> OpenH264 (Multi-Slice)`.
2. **Multi-Slice OpenH264 (`crates/host-win/src/encode/openh264_mt.rs`):** Call `openh264-sys2` (`ISVCEncoder`, `SEncParamExt`) with `SM_FIXEDSLCNUM_SLICE`, `iMultipleThreadIdc = 2..8` (capped safely by GOM rows for small test frames), `LOW_COMPLEXITY`, `bEnableFrameSkip = true`, `iMaxQp = 40`, `WELS_LOG_ERROR` prior to `InitializeExt`, and live `ENCODER_OPTION_BITRATE` updates.
3. **Wire Protocol v2 Extensions (`crates/proto/src/control.rs`):** Add `Ping { ts_us: u64 }` (`0x0F`), `Pong { ts_us: u64 }` (`0x10`, auto-replied inside `HostSession`), and `RequestLog` (`0x11`, host -> client -> JNI `STATE_LOG_REQUEST = 12` so Windows "Xuất log" auto-pulls Android logs).
4. **Real Telemetry & Live Controls:** `LowLatencyDecoder` tracks per-frame `queueInputBuffer -> onOutputBufferAvailable` decode latency + `codecThreadTid` (wired to `AdpfSession` and `nativeReportDecodeUs`); `rv-android-core` sends `FeedbackStats` every 1 s with real `rtt_us`, `loss_permille`, `recv_kbps`, `decode_us`; Android HUD displays `RTT · Dec · Mbps`; Windows GUI logs periodic `stats` and supports `set_resolution 0x0` reset.
5. **LAN End-to-End & Security Hardening:** Unify LAN UDP + discovery port to `47800`, run `discovery::run_beacon` on host while filtering `RVD1` beacons and guarding `SharedPeer` once established, implement `nativeConnect` + `nativeStartDiscovery` in `rv-android-core` + `PinDialog` on Android. Add `SendSAS(FALSE)` via System32 `sas.dll` (and `SoftwareSASGeneration = 1` on service install), PE icon + `PerMonitorV2` manifest (`apps/win-ui/build.rs`), Win32 system tray (`tray.rs`), Job Object (`job.rs`), `%LOCALAPPDATA%` `wdi-simple` extraction, and SHA-256 verification on CI release + Windows/Android updaters.

## Consequences
- APK and `RemoteView.exe` continue to be released from the same commit (protocol v2 additive op codes `0x0F..0x11`).
- Native AMF, `SendSAS`, Win32 tray, and PE manifest/icon are verified via Rust tests + `x86_64-pc-windows-gnu` cross-compile + CI build, and still require real-hardware verification on Vivobook S14 OLED + Galaxy Tab S10+.
