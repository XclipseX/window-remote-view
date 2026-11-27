---
type: index
updated: 2026-11-27
---
# Project Index (Map of Content)

> AI agents: read this note first, then the latest note in sessions/.

## Architecture

## Decisions
- [[decisions/ADR-0002-large-frames]] – protocol v2, FEC theo block
- [[decisions/ADR-0003-sudovda-virtual-display]] – RemoteView tự tạo màn hình ảo (SudoVDA), hết màn đen khi không có TeamViewer
- [[decisions/ADR-0004-system-service-lock-screen]] – dịch vụ SYSTEM `RemoteViewSvc` để xem/điều khiển màn hình khóa
- [[decisions/ADR-0005-native-amf-and-telemetry]] – AMD AMF DX11 gốc, OpenH264 đa slice, đo RTT/decode_ms, LAN UDP + discovery, SendSAS, tray/icon/SHA-256
- [[decisions/ADR-0006-always-on-boot-daemon]] – `RemoteViewSvc` tự chạy ngay từ Winlogon lúc boot, `%ProgramData%` dùng chung, bền bỉ chuyển desktop/sleep-wake, Android tự attach & reconnect tức thì

## Known bugs / gotchas
- Tab S10+ Android 17 không bật được USB tethering
- [[bugs/2026-11-27-low-fps-timer-sw-decoder]] – tối ưu FPS 120 Hz (timer 1ms, tách vòng lặp session, AMF/MF speed, SudoVDA 120Hz, FramePresenter, xuất log GUI)

## Recent sessions
- Bàn giao Gemini: [[../handoff/01-BAO-CAO-BAN-GIAO]]
- [[sessions/2026-11-27]] (màn ảo SudoVDA, dịch vụ SYSTEM, AMD AMF gốc, OpenH264 đa slice, RTT/decode HUD, LAN UDP, tối ưu FPS 120Hz)
- [[sessions/2026-11-26]] (USB auto-pair, single exe)
