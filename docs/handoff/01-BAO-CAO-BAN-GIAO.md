# Báo cáo bàn giao dự án RemoteView

> Người nhận: Gemini (Antigravity) và các agent con. Đọc `docs/handoff/00-PROMPT-GEMINI.md` trước. Tài liệu này mô tả **toàn cảnh dự án, yêu cầu của chủ dự án, kiến trúc, hiện trạng đã kiểm chứng trên thiết bị thật, lỗi còn tồn tại (kèm log mới nhất) và kế hoạch làm tiếp**. Ngày lập: 2026-11-27. Commit tham chiếu: `4d280bd`.

---

## 1. Mục tiêu và yêu cầu của chủ dự án

Chủ dự án muốn một ứng dụng **giống TeamViewer nhưng tốt hơn ở những điểm mình quan tâm**, hoàn toàn **miễn phí, mã nguồn mở**, dễ dùng, với hai chức năng chính:

1. **Cài được trên Windows và Android.**
2. **Android hiển thị và điều khiển màn hình laptop Windows** một cách dễ dàng.

Yêu cầu chi tiết, theo đúng thứ tự chủ dự án đã nêu trong quá trình làm việc:

- **Tối ưu độ trễ, điện năng, chất lượng hình ảnh, FPS.** Không dùng thuật toán độc quyền ngốn tài nguyên. Dùng **codec mở** (H.264/HEVC/AV1), hỗ trợ **mã hóa/giải mã phần cứng** và **dự phòng phần mềm**.
- **Tùy chỉnh độ phân giải, FPS, chất lượng** bằng thanh trượt 1–100%.
- **Đầy đủ nhập liệu:** bàn phím (cả tiếng Việt Telex), cảm ứng đa điểm, rê chuột kiểu trackpad, chuột Bluetooth, **S Pen** có lực nhấn.
- **Kết nối:** USB (ưu tiên, độ trễ thấp nhất), LAN và qua server tự host miễn phí. Phải tự chuyển đổi.
- **USB phải "cắm là chạy":** không bắt bật USB tethering, không bắt nhập PIN khi dùng cáp. Hai bên **tự nhận diện tên máy** để chọn kết nối. **Không phải tự cài Zadig hay chương trình phụ** — mọi thứ gói trong **một file `RemoteView.exe`** và **một file APK**.
- **Điều quan trọng nhất (chủ dự án nhấn mạnh):** Tab phải **hiển thị được nội dung từ laptop**. Không quan tâm đó là màn hình chính hay phụ; thực tế laptop **hỏng màn**, chủ dự án đang dùng TeamViewer để xem màn hình laptop trên máy tính bảng, và muốn RemoteView thay thế việc đó với chất lượng tốt hơn.
- **Đúng tỷ lệ:** máy tính bảng 16:10 nên hình phải là 16:10 (không kéo giãn, hạn chế viền đen).
- **Thấy con trỏ chuột thật** của laptop, đúng vị trí.
- **Giao diện** đẹp, trực quan, **responsive cho máy tính bảng**; app Windows có UI/UX đầy đủ.
- **Tự kiểm tra và tải cập nhật** trong cả hai app.
- **Log** ghi từ lúc mở app, **xuất file** dễ gửi; Android gửi log lên Windows qua cáp để gom một file.
- **Chẩn đoán** (Android) và **stress test** giải mã (đã làm, ưu tiên thấp hơn tính năng chính).
- **APK có chữ ký** cố định để cài đè được.
- **Build trên GitHub Actions**, có link tải trực tiếp; build càng nhanh càng tốt.
- Làm việc theo hướng **chia nhiều agent**, có dùng skill, graphify, Obsidian.

## 2. Thiết bị thật của chủ dự án

| Thiết bị | Thông tin đã xác nhận từ log/chẩn đoán |
|---|---|
| **Laptop ASUS Vivobook S14 OLED** | Ryzen AI 9 HX 370, **Radeon 890M** (VCN 4.0.5), Windows 11 build 26200. **Màn hình laptop hỏng.** Hiện chỉ có **một output: `\\.\DISPLAY10` 1920×1080 "TV Virtual" — TeamViewer Virtual Monitor Adapter** (màn hình ảo do TeamViewer tạo). Không dùng spacedesk (đã nghi nhầm trước đó). |
| **Galaxy Tab S10+** SM-X826B | Dimensity 9300+ (MT6989), **Android 17 (API 37)**, màn **2800×1752 16:10**, 120 Hz. Có `c2.mtk.{avc,hevc,av1}.decoder.lowlatency`. USB tethering bị One UI từ chối (không quan trọng nữa vì đã dùng AOA). |
| **Galaxy S26+** SM-S947B | Exynos 2600, Android 16, `c2.exynos.*`. Stress test: trần HEVC 2800×1752 ≈ **560 fps** tổng (14 decoder song song), không bóp xung. |

Cáp: UGREEN USB-C 3.x 10 Gbps. Laptop đã có driver WinUSB cho `18D1:2D00` (từ Zadig trước đây); bản mới có tự cài qua libwdi nhưng chưa được kiểm chứng trên máy sạch.

## 3. Kiến trúc

Monorepo Rust workspace + app Android Kotlin:

```
crates/proto          rv-proto      Wire protocol v2 (header 12 B, video/audio/input/control). ControlMessage op codes.
crates/crypto         rv-crypto     Noise XX/XXpsk3/IK, OTP→PSK (argon2id), seal/open UDP, replay window, AttemptLimiter.
crates/transport      rv-transport  FEC Reed-Solomon theo khối (≤64 data shard/khối, ~18,7 MB/khung), LAN discovery, pacer, path race (LAN→IPv6 P2P→IPv4→relay), USB-tether detect, TCP/443 tunnel.
crates/session        rv-session    HostSession/ClientSession sans-IO: handshake, keepalive 1 s, timeout 5 s, reconnect, StreamConfig, slider_to_bitrate.
crates/usb-link       rv-usb-link   AOA host (libusb/rusb), framing u32 BE length, ADB fallback.
crates/host-win       rv-host-win   Engine host Windows: capture (DXGI + GDI fallback), encode (Media Foundation HW → OpenH264), audio WASAPI+Opus, input injection, cursor, JSON control. Bin `rv-host` (debug CLI).
crates/android-core   rv-android-core  librv_core.so (JNI) chạy ClientSession trên Android.
apps/win-ui           rv-win-ui     RemoteView.exe (egui). Tự chạy engine bằng `RemoteView.exe --host-worker` (tiến trình con), nhúng wdi-simple.exe, tự cập nhật, log file, xuất zip.
apps/android          Kotlin/Compose: AOA link, decoder ladder theo hãng, UI adaptive, chẩn đoán, stress test, updater, log.
server/signal-relay   rv-signal-relay  ID/signaling + relay UDP/TCP, giới hạn 25 Mbps/phiên, Docker.
```

**Luồng USB hiện tại:**
1. GUI bấm "Bắt đầu chia sẻ" → spawn worker.
2. Worker tìm thiết bị Android, gửi AOA strings: manufacturer `RemoteView`, model `RemoteViewHost`, description = tên máy tính, version `2`, **serial = token 8 số lưu tại `%APPDATA%\RemoteView\usb_token`**.
3. Android nhận `USB_ACCESSORY_ATTACHED` → mở accessory → dùng serial làm OTP → Noise XXpsk3 handshake.
4. Client gửi Hello (codec mask), DeviceInfo (tên máy).
5. Host gửi `DisplayConfigChange` + `StreamConfig{codec,w,h,fps,kbps}`, rồi video (FEC shards), audio (Opus), `CursorShape`/`CursorPos`.
6. Android gửi input (touch/pen/mouse/key/text), `LogChunk` khi upload log.

**Hợp đồng giữa các phần (đổi phải cập nhật hai phía + ADR):**
- Op code `ControlMessage` trong `crates/proto` (0x01–0x0E; `CursorPos`=0x0D, `LogChunk`=0x0E, `DeviceInfo`=0x0C, `StreamConfig`=0x0A, `Keepalive`=0x0B).
- `crates/host-win/CONTROL_PROTOCOL.md`: JSON lines GUI↔worker (status/stats/devices/log; lệnh set_quality/set_fps/set_resolution/request_idr/new_otp/connect/stop; tham số `--monitor`).
- JNI: `apps/android/.../NativeBridge.kt` ↔ `crates/android-core/src/jni_bridge.rs` (callbacks `onConnectionStateFromNative`, `onSendDatagramFromNative`, video/audio, `onCursorShapeFromNative`, `onCursorPosFromNative`; hàm `nativeConnectLink`, `nativePushDatagram`, `nativeSendTouch`…, `nativeUploadLog`). Mã state: 0 connecting, 1 established, 2 closed, 4 disconnected, 10 display config, 11 stream config.

## 4. Build, phát hành, kiểm tra

- `.github/workflows/ci.yml`: job `rust` (Linux test+clippy), `wdi` (libwdi → `wdi-simple.exe`, có cache `wdi-simple-v1`), `windows-app` (RemoteView.exe, nhúng wdi, `RV_BUILD_SHA`), `android` (cargo-ndk arm64 → librv_core.so, Gradle assembleDebug, verify chữ ký + .so), `publish` (Release `latest-build`, xóa asset cũ). `concurrency` hủy run cũ. Thời gian ~5 phút.
- APK ký bằng `apps/android/app/debug.keystore` (công khai, chỉ cho thử nghiệm; ADR-0001). `versionCode` = số run CI.
- Cập nhật tự động: app so SHA trong mô tả Release ("Bản build tự động từ commit <sha>") với SHA build. Repo công khai nên API đọc được.
- Kiểm tra cục bộ: `cargo test --workspace` (155 test), `cargo clippy --workspace --all-targets`, `cargo check -p rv-host-win -p rv-win-ui --target x86_64-pc-windows-gnu`. Kotlin chỉ biên dịch trên CI.
- **Bài học CI:** chú thích KDoc chứa `*/` làm vỡ file Kotlin; clippy chặn hằng số kiểu `3.14`; build cũ có thể đè Release nếu không có `concurrency`; tên biến sai giữa các file do agent khác viết — luôn đọc log job sau khi push.

## 5. Lịch sử lỗi trên thiết bị thật (đã sửa)

| # | Triệu chứng | Nguyên nhân gốc | Sửa |
|---|---|---|---|
| 1 | Tab không gửi handshake | Kotlin chặn ở hộp thoại PIN; đọc AOA bằng buffer nhỏ làm mất gói; thứ tự khởi tạo native | Auto OTP từ serial, đọc 16 KiB, sắp xếp lại khởi tạo |
| 2 | Host không nhận byte nào (`usb_raw_in=0`) | Worker tự mở accessory khi quét thiết bị + reset accessory "cũ" | Token cố định, bỏ reset, chỉ một chỗ mở USB, GUI không mở USB khi worker chạy |
| 3 | StreamConfig gửi lại mỗi 0,5 s, không có video | AMD MFT trả `MF_E_NOTACCEPTING` mọi khung; bộ đếm lỗi bị reset khi dựng lại encoder | Drain + retry, không reset streak, supervisor fallback |
| 4 | Capture chết sau rebuild (`0x887A0002`) | Cache ordinal adapter/output | Re-enumerate mỗi lần, GDI fallback |
| 5 | "Access is denied" khắp nơi | Laptop khóa màn hình (secure desktop) | Keep-awake, phát hiện khóa, gửi khung cuối |
| 6 | Log mất phần đầu | Ring buffer nhỏ | Log file từ lúc mở app, xuất zip, Android upload |
| 7 | Kéo giãn 16:9 trên Tab 16:10 | Android scale full | Letterbox theo StreamConfig |
| 8 | Không có chuột | DXGI không vẽ cursor | CursorShape + CursorPos + overlay Android |

Chi tiết từng lỗi: `docs/vault/bugs/`. Quyết định: `docs/vault/decisions/` (ADR-0001 ký APK, ADR-0002 khung lớn/FEC theo khối).

## 6. Phân tích log mới nhất (commit 4d280bd, 2026-11-27 07:54)

Phản hồi chủ dự án: *"Chuột thì thấy nhưng không rõ chuột thật hay mô phỏng, hơi lệch. OpenH264 1080p60 vừa lag vừa không nét. Kết nối kỳ cục: nhấn kết nối trên PC, mobile tự gửi, thấy bắt tay rồi nhưng không lên hình, màn hình ghi 'đang kết nối'; vuốt thoát app (không xóa đa nhiệm) rồi vào lại thì lên hình nhưng rất lag. Laptop hỏng màn, đang dùng TeamViewer để xuất màn hình, không phải spacedesk."*

Log Windows (`remoteview-20261127-075413.log`) và Android (`app-20261127-075214.log`) cho thấy:

### 6.1. Màn hình duy nhất là màn ảo TeamViewer 1920×1080
`Output #0: \\.\DISPLAY10 1920x1080 "TV Virtual" (TeamViewer Virtual Monitor Adapter) primary=true`. Đây là **nguồn của tỷ lệ 16:9**. Vì màn laptop hỏng, Windows chỉ còn màn ảo của TeamViewer. **Không thể có 16:10 đúng nghĩa nếu không tự tạo màn hình ảo.**
→ **Việc ưu tiên số 1:** tích hợp **driver màn hình ảo IddCx** (ví dụ dự án mã nguồn mở Virtual-Display-Driver / SudoVDA, hoặc tự viết bằng IddCx sample của Microsoft, ký driver là thách thức lớn nhất) để tạo màn **2800×1752 @60/120** đúng kích thước Tab, đặt làm màn chính khi chia sẻ, gỡ khi dừng. Tạm thời có thể đổi độ phân giải màn ảo TeamViewer sang 1920×1200 nếu nó hỗ trợ (ChangeDisplaySettingsEx) — nên thử và log danh sách mode.

### 6.2. Encoder phần cứng AMD vẫn chết, phải chạy OpenH264
`mft event: METransformNeedInput at +20 ms` → sau `ProcessInput` đầu tiên **không còn sự kiện nào** (không NeedInput, không HaveOutput) → `no METransformNeedInput within 300 ms` ×2 → fallback OpenH264 1080p60 14,7 Mbps. OpenH264 cấu hình sai: cảnh báo "bitrate can't be controlled … without enabling skip frame", AdaptiveQuant/BackgroundDetection bị tắt cho screen content.
→ Đây là **nguyên nhân lag + mờ**. Hướng xử lý (theo thứ tự):
1. **AMF native** qua AMF SDK (C++ FFI hoặc crate bindings) — đúng như báo cáo nghiên cứu đề xuất (HEVC ULTRA_LOW_LATENCY, không B-frame, CBR+HRD, VBV 1,5×, SPEED preset). Radeon 890M đủ sức 1080p120/1440p120.
2. Nếu vẫn dùng MF: kiểm tra (a) có set `MF_LOW_LATENCY`/`CODECAPI_AVLowLatencyMode` *trước* `SetOutputType`; (b) input là texture NV12 từ **cùng D3D device** với `IMFDXGIDeviceManager`; (c) sample time/duration hợp lệ; (d) thử `MFT_MESSAGE_COMMAND_DRAIN` sau khung đầu; (e) log `MF_E_TRANSFORM_NEED_MORE_INPUT`. Có thể AMD MFT cần **≥2 khung** trước khi xuất — cần đọc tiếp event sau ProcessInput thay vì kết luận sau 300 ms.
3. Thử **D3D12 Video Encode** hoặc **FFmpeg `hevc_amf`** (nhanh nhất để có hình phần cứng, đổi lại thêm dependency).
4. Cải thiện OpenH264 khi buộc phải dùng: `RC_BITRATE_MODE` + `bEnableFrameSkip=true`, `iUsageType=SCREEN_CONTENT_REAL_TIME`, 1080p**30**, nhiều thread, preset nhanh; hoặc **x264** (`ultrafast`, `zerolatency`, CRF) cho chất lượng tốt hơn ở cùng CPU.

### 6.3. Bug Android: không áp dụng StreamConfig khi đang mở
Android khởi tạo decoder **AV1 2800×1752** trước khi có StreamConfig (`decoder start=true name=c2.mtk.av1.decoder.lowlatency 2800x1752@60`), sau đó nhận `hevc 1920x1080@120` rồi `h264 1920x1080@60` nhưng **không dựng lại decoder** — chỉ khi người dùng thoát/vào lại app (surface tạo lại) mới thấy `decoder start … avc 1920x1080@60`. Đây chính là hiện tượng "đã bắt tay nhưng chưa lên hình, thoát vào lại thì lên".
→ Sửa `SessionController`: **không khởi tạo decoder trước StreamConfig**; khi nhận StreamConfig mà mime/kích thước khác decoder hiện tại thì `stopVideo()`+`startVideo()` **ngay trên surface hiện có**; sau khi decoder mới chạy, `nativeRequestIdr`. Thêm test thủ công: đổi codec giữa phiên.
→ Đồng thời: Android quảng bá `codecs mask 0x7` (gồm AV1) — với Tab S10+ nên **không quảng bá AV1** ở 120 Hz (đã ghi trong nghiên cứu), và host nên ưu tiên HEVC > H.264.

### 6.4. Con trỏ "hơi lệch"
Overlay Android vẽ theo `CursorPos` chuẩn hóa trên khung capture; ở chế độ trackpad còn có **dự đoán cục bộ** rồi bị vị trí thật ghi đè → nhảy/lệch. Kiểm tra: (a) DPI awareness của worker (per-monitor v2) để `GetCursorInfo` trả tọa độ vật lý trên đúng màn ảo; (b) tọa độ tương đối với rect của output đang capture (màn ảo TeamViewer ở (0,0)); (c) scale overlay theo content rect + hotspot; (d) thêm tùy chọn **vẽ con trỏ vào khung hình phía host** (DXGI pointer shape composite) để không lệch tuyệt đối; giữ overlay cho chế độ độ trễ thấp.

### 6.5. Khác
- `rv-usb: accessory serial "R52Y106D7TR" differs from our pairing token` — accessory còn từ build cũ, nhưng handshake vẫn thành công (Android dùng serial thật từ lần switch mới); chỉ cần log rõ hơn.
- Phiên kết thúc với `native state=4` (disconnected) lúc 07:56:10 rồi đóng — kiểm tra keepalive khi Android ở nền (app vào background thì có thể thread bị giới hạn → host timeout).
- Peer hiển thị fingerprint `1bacb010c72f` trước khi DeviceInfo đến — bình thường.

## 7. Việc còn lại, theo thứ tự ưu tiên

**P0 — Tab phải hiện màn hình laptop mượt, đúng tỷ lệ**
1. Sửa bug Android StreamConfig (6.3). *Nhỏ, làm ngay.*
2. Encoder phần cứng AMD chạy được (6.2): AMF native hoặc FFmpeg AMF; mục tiêu HEVC 1080p60 ≤ 8 ms/khung, sau đó 2800×1752@60/120.
3. Màn hình ảo IddCx đúng kích thước Tab (6.1), xử lý ký driver (test-signing không chấp nhận được cho người dùng cuối → cần driver đã ký sẵn của dự án mã nguồn mở hoặc chứng chỉ EV/attestation signing).
4. Con trỏ chính xác (6.4).
5. Đo độ trễ đầu-cuối thật (timestamp capture → decode → present, hiển thị trên overlay).

**P1 — Trải nghiệm**
6. Chạm/S Pen/bàn phím/Telex kiểm chứng trên Tab; Ctrl+Alt+Del cần Windows Service (SYSTEM) — đồng thời giải quyết capture/inject trên màn khóa và UAC.
7. Âm thanh: xác nhận Opus phát trên Android (MediaCodec audio/opus), độ trễ.
8. App Windows: tray icon, khởi động cùng Windows, icon .exe nhúng, cài đặt (MSIX/WiX) để tránh SmartScreen.
9. Kết nối lại mượt khi rút/cắm cáp, khi Android vào nền.

**P2 — Mở rộng**
10. LAN (mDNS + PIN/QR) và server relay (Docker trên Oracle Cloud Free) hoàn chỉnh, end-to-end test.
11. Chế độ "Mở rộng màn hình" (extend) dùng chung driver IddCx.
12. S26+ (Exynos) kiểm chứng; AV1 cho S26+ khi đo được < 8 ms.
13. Khóa ký phát hành riêng (GitHub Secrets) khi lên Play/F-Droid.

## 8. Đề xuất chia agent (10–20)

| Agent | Vùng sở hữu | Việc |
|---|---|---|
| A1 Lead/integrator | toàn repo (merge), `docs/vault` | kế hoạch, gộp, CI, liên lạc chủ dự án |
| A2 Android session | `apps/android/.../session`, `decoder` | bug StreamConfig, reconnect, background |
| A3 Android UI | `apps/android/.../ui` | overlay, cursor, cài đặt, chẩn đoán |
| A4 Android input | `apps/android/.../input` | touch/pen/keyboard/Telex |
| A5 AMF encoder | `crates/host-win/src/encode/amf*` | AMF native HEVC/H.264 |
| A6 MF/FFmpeg encoder | `crates/host-win/src/encode/mf.rs`, `ffmpeg*` | phương án dự phòng phần cứng |
| A7 Software encoder | `encode/openh264*`, `x264*` | tối ưu phần mềm |
| A8 Capture & cursor | `crates/host-win/src/capture*`, `cursor*` | DXGI, cursor composite, DPI |
| A9 Virtual display | `crates/vdisplay` (mới), driver | IddCx, ký driver, API tạo/xóa màn |
| A10 Windows service | `crates/host-svc` (mới) | SYSTEM service, secure desktop, SAS |
| A11 Win UI | `apps/win-ui` | UX, tray, installer, updater |
| A12 USB link | `crates/usb-link` | AOA ổn định, driver auto-install trên máy sạch |
| A13 Protocol/session | `crates/proto`, `crates/session` | hợp đồng, đo độ trễ, keepalive |
| A14 Android core JNI | `crates/android-core` | JNI, callback, upload log |
| A15 Network LAN/relay | `crates/transport`, `server/` | LAN/relay end-to-end |
| A16 CI/Release | `.github/` | tốc độ build, ký, phát hành |
| A17 QA/log analyst | `docs/vault/bugs`, scripts | đọc log chủ dự án gửi, tái hiện, viết test |

Mỗi agent: đọc vault + graphify trước, chỉ sửa vùng của mình, chạy test vùng mình, ghi ADR/bug, báo cáo ngắn cho A1.

## 9. Lưu ý vận hành với chủ dự án

- Chủ dự án test trên máy thật và gửi log; **luôn nói rõ cái gì chưa kiểm chứng**. Đừng đoán nguyên nhân thiết bị (ví dụ từng nghi spacedesk nhưng thực tế là TeamViewer) — hỏi hoặc đọc log danh sách output.
- Trả lời bằng tiếng Việt, ngắn, có link tải khi có bản mới; cập nhật qua nút trong app.
- Mỗi bản phải phát hành APK và RemoteView.exe **cùng commit**.
- Hướng dẫn gửi log: Tab → bảng gỡ lỗi → "Gửi log lên máy tính"; laptop → Nhật ký → "Xuất log" → file zip trên Desktop.
