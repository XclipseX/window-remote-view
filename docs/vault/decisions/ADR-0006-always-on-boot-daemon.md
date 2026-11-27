---
type: decision
id: ADR-0006
date: 2026-11-27
status: accepted
tags: [windows-service, winlogon, usb-aoa, android, boot, spacedesk]
---
# ADR-0006: Always-On Boot Daemon (`RemoteViewSvc`), Shared `%ProgramData%` State & Instant USB Handover

## Context
Trên laptop hỏng màn hình vật lý (ASUS Vivobook S14 OLED), trải nghiệm chuẩn Spacedesk đòi hỏi bật laptop lên và cắm cáp USB là máy tính bảng lập tức hiện màn hình khóa Windows (Winlogon) để gõ PIN/mật khẩu đăng nhập rồi chuyển thẳng sang Desktop — không cần mở GUI trước, không bị đứt hình khi GUI mở/đóng, và không phải bấm kết nối lại trên Android.

Kiểm toán 5 tầng đã phát hiện 4 nhóm nút thắt gốc:
1. `RemoteViewSvc` trước đây chỉ ngồi chờ `ConnectNamedPipe` đợi GUI gửi `spawn`, không tự chạy ở Winlogon lúc boot, và tự giết worker ngay khi GUI đóng.
2. Lệch hồ sơ `LocalSystem` vs User (`%APPDATA%`/`%LOCALAPPDATA%`) gây ra 2 `usb_token` khác nhau giữa Winlogon và Desktop (AOA sai PSK → bị `AttemptLimiter` khóa 30s–1h) và làm mất toàn bộ log runtime của service khỏi "Xuất log".
3. Chuyển desktop `Winlogon ↔ Default` / UAC tạo `ID3D11Device` mới nhưng `video_thread` nhét vào `AmfEncoder` cũ → `E_INVALIDARG` rồi blacklist vĩnh viễn AMF; `WinInjector` tắt luôn chuột/bàn phím khi `CreateSyntheticPointerDevice` fail trên Winlogon; `audio_thread` chết vĩnh viễn sau sleep/wake.
4. `UsbDatagramLink` rò rỉ luồng đọc giữ WinUSB khi reopen; `aoa.rs` coi `Access` là thiếu driver làm gọi nhầm `wdi-simple` (`-11 = WDI_ERROR_RESOURCE`); Android xếp hàng nhiều `msg1` cũ và phá hủy `Initiator` gây `handshake rate limited` + mất 5–7s hoặc kẹt `HomeScreen`.

## Decision
1. **Always-On Boot Daemon (`apps/win-ui/src/service.rs`, `service_proto.rs`, `host.rs`, `app.rs`):**
   - `RemoteViewSvc` tự khởi chạy và giám sát 1 `--host-worker` duy nhất ngay từ lúc boot (khi `WTSGetActiveConsoleSessionId()` có phiên console), đọc cấu hình dùng chung `%ProgramData%\RemoteView\service_config.json`, đăng ký `SERVICE_ACCEPT_SESSIONCHANGE` + `SERVICE_ACCEPT_POWEREVENT` + SCM failure recovery (`SC_ACTION_RESTART`), và xử lý `SendSAS(FALSE)` trực tiếp từ Session 0 qua sự kiện `sas_req`.
   - Giao thức pipe hỗ trợ `attach` / `detach` / `stop_sharing` + `EventCache`: GUI mở lên gắn thẳng vào worker đang chạy (zero disconnect); đóng GUI chỉ `detach` để giữ nguyên màn hình máy tính bảng; thêm Single-Instance Mutex `Local\RemoteViewGUI_SingleInstance`.
2. **Thống nhất lưu trữ toàn máy `%ProgramData%\RemoteView\` (`control.rs`, `logfile.rs`, `config.rs`, `logexport.rs`):**
   - `usb_token` (tự migrate từ `%APPDATA%`), `service_config.json`, và `logs\remoteview-service-*.log` đều nằm ở `%ProgramData%\RemoteView\`; `logexport` gom cả `%LOCALAPPDATA%` và `%ProgramData%`.
3. **Bền bỉ chuyển Desktop `Winlogon ↔ Default` & Sleep/Wake (`engine.rs`, `capture.rs`, `capture_win.rs`, `sudovda.rs`, `input_win.rs`, `session_state.rs`):**
   - Theo dõi `device_id` của `ID3D11Device` và GPU↔CPU để dựng lại encoder sạch không phạt `HwSupervisor`; giao frame tĩnh đầu tiên trên Winlogon; `prefer_virtual` không nhảy về màn vật lý hỏng; `SudoVdaMonitor::reapply` + fallback dynamic mode trên Winlogon; `WinInjector` giữ `touch_dev`/`pen_dev` là `Option`, tự tạo lại khi đổi desktop, fallback chạm 1 ngón sang chuột trên Winlogon, gửi ASCII bằng `KEYEVENTF_SCANCODE`; `audio_thread` tự hồi phục.
4. **USB AOA & Android Instant Reconnect (`crates/usb-link`, `crates/session`, `crates/android-core`, `apps/android`):**
   - `UsbDatagramLink::Drop` join sạch thread đọc/ghi nhả handle WinUSB; `aoa.rs` không giữ `USB_LOCK` khi sleep, tạo `Context` mới khi `wait_reenumerate`, phân biệt `Access/Busy` vs `NotSupported`; `driver.rs` truyền `--dest` + `lpDirectory` và giải mã mã lỗi `WDI_ERROR_*`.
   - `ClientSession::start()` tái sử dụng `Initiator` + `m1` khi gửi lại; thêm `HS_STEP_RESET (0xFF)` + `USB_HANDSHAKE_MIN_INTERVAL=50ms` + `USB_IDLE_TIMEOUT=2.5s` trên link USB.
   - Android thêm `showWhenLocked`/`turnScreenOn`, retry `openAccessory` 8 lần + khóa `fdLock` chống leak `open_excl`, khử trùng lặp `0xA5`, tách `nativeDisconnect` ra nền, tự reconnect backoff khi host restart mà cáp vẫn cắm.

## Consequences
- Cắm cáp lúc bật máy → hiện thẳng Winlogon → gõ PIN/mật khẩu đăng nhập → chuyển sang Desktop & mở/đóng GUI đều liền mạch như Spacedesk.
- Người dùng cần nhấn **"Cập nhật dịch vụ"** 1 lần sau khi tải `RemoteView.exe` mới để cập nhật cấu hình SCM recovery và khởi động lại `RemoteViewSvc` với bản daemon mới.
