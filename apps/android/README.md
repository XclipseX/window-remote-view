# Remote View — client Android (CHƯA BIÊN DỊCH, CHƯA KIỂM THỬ)

> **Cảnh báo:** mã trong thư mục này được viết trong container không có Android SDK nên
> **chưa từng được biên dịch hay chạy**. Hãy coi đây là khung/bản nháp "gần biên dịch được":
> khi mở bằng Android Studio sẽ còn lỗi cú pháp/import nhỏ cần sửa. Mọi số liệu về Exynos 2600 là
> suy luận từ Exynos 2400/2500 (xem `docs/research-2-ket-qua-gemini.md`) và **phải đo trên máy thật**.

Máy đích: Galaxy S26+ (Exynos 2600, `c2.exynos.*`) và Galaxy Tab S10+ (Dimensity 9300+, `c2.mtk.*`).
`minSdk 26`, `targetSdk/compileSdk 36`, Kotlin 2.1, AGP 8.10, Jetpack Compose (Material 3).

## Cách dựng

1. Mở thư mục `apps/android/` bằng Android Studio (Ladybug trở lên) hoặc chạy `gradle wrapper` rồi `./gradlew :app:assembleDebug`.
   (Chưa có Gradle wrapper vì không tạo được jar trong container.)
2. Lõi Rust (`crates/android-core`, thư viện `librv_core.so`) build bằng cargo-ndk, CI làm sẵn:
   `cargo ndk -t arm64-v8a -o apps/android/app/src/main/jniLibs build --release -p rv-android-core`
   (cần `rustup target add aarch64-linux-android`, `cargo install cargo-ndk`, `ANDROID_NDK_HOME`). Gradle tự đóng gói
   `src/main/jniLibs/arm64-v8a/librv_core.so` (thư mục bị .gitignore). Thiếu thư viện thì `NativeBridge.available == false`
   và UI vẫn chạy, lệnh native là no-op.

### Lõi Rust trong APK (chỉ đường USB)

- `NativeBridge.nativeConnectLink(helloJson)` tạo phiên `rv-session` `ClientSession` chạy ở thread riêng của Rust; datagram từ
  AOA/ADB đi vào bằng `nativePushDatagram`, gói gửi đi quay về Kotlin qua `onSendDatagramFromNative`. Video (Annex-B, H.264),
  gói Opus thô, trạng thái, thống kê trả lên qua các hàm `on*FromNative`. `nativeConnect` (LAN/server) **chưa cài** (trả 0).
- **Ghép đôi:** host in `OTP: dddddd` khi khởi động (hoặc cố định bằng `rv-host --otp 123456`). Người dùng nhập mã này (PIN 6–9 số)
  ở hộp thoại khi bấm "Màn hình phụ (USB)"; được lưu trong SharedPreferences (`usb_otp`) và gửi cho Rust trong trường `"otp"` của
  helloJson. Cả hai phía dẫn khóa PSK bằng Argon2id(OTP, `host_id`), `host_id` là hằng cố định `rv_session::USB_HOST_ID`
  (`rv-host-win-0001`) vì chưa có hello trước bắt tay. Sai PIN: host im lặng, sau ~4 s app báo lỗi và hỏi lại PIN.
- Hello (độ phân giải gốc + tần số quét) gửi ngay sau bắt tay; host hiện chưa dùng nó (đọc `--mode` từ dòng lệnh).
- Opus giải mã trong Kotlin bằng `MediaCodec` "audio/opus" (`AudioPlayer`); không build libopus cho Android.
- Host chỉ nhận một lần bắt tay mỗi lần chạy: nối lại sau khi rút cáp cần khởi động lại `rv-host`.
## Cấu trúc (`app/src/main/java/com/remoteview/client/`)

| Đường dẫn | Vai trò |
|---|---|
| `MainActivity.kt` | Điều hướng Home / Cài đặt / Phiên; xin quyền; lock Wi-Fi theo vòng đời; PauseVideo ở `onStop`. |
| `NativeBridge.kt` | Khai báo JNI tới Rust (`external fun`), callback ngược, `NativeInputSink`. Cài đặt: `crates/android-core`. |
| `AppSettings.kt` | `StreamSettings` + lưu SharedPreferences. |
| `decoder/LowLatencyDecoder.kt` | Chọn codec (ưu tiên HEVC; AV1 chỉ khi cờ cho phép, tránh AV1 trên `c2.mtk` ở ≥ 90 fps; loại `unisoc` AV1 giả HW), MediaCodec async, thang fallback theo hãng (mỗi bậc một `MediaFormat` mới, lọc khóa `vendor.*` bằng `getSupportedVendorParameters()` ở API 31+). |
| `decoder/StallWatchdog.kt` | Input chảy mà 1 s không có output → flush + xin IDR; 5 s → tạo lại codec (leo bậc ladder). |
| `render/FramePresenter.kt` | Choreographer + "latest frame": bỏ khung cũ `releaseOutputBuffer(idx,false)`, render khung mới nhất; chế độ Direct Present; báo idle (>500 ms). |
| `render/DisplayLock.kt` | `Surface.setFrameRate(fps, FIXED_SOURCE, CHANGE_FRAME_RATE_ALWAYS)` + `preferredDisplayModeId` (refresh cao nhất ở độ phân giải gốc); hạ 10 Hz khi hình đứng yên. |
| `perf/AdpfSession.kt` | PerformanceHintManager: target 6,5 ms @120, 12 ms @60. |
| `perf/ThermalGovernor.kt` | Headroom < 0,75 đầy đủ; 0,75–0,9 bitrate 70%; ≥ 0,9 hạ 60 fps (có hysteresis). |
| `input/` | `TouchGestureMapper` (chạm=nhấp trái, giữ 400 ms/2 ngón chạm=nhấp phải, 2 ngón cuộn, chụm=zoom cục bộ, Trackpad/Direct), `StylusMapper` (áp lực 0..1024, nghiêng, hover, nút bên, tẩy), `PointerCaptureController` (chuột BT/OTG), `KeyboardBridge` (phím vật lý → scancode Set 1 từ `getScanCode()`; IME ảo → `BaseInputConnection` gửi Unicode theo diff, kể cả Telex đang composing; chế độ Raw typing), `ScanCodeMap`, `RemoteInput`. |
| `net/` | `NetworkWatcher` (NetworkCallback; Wi-Fi RSSI < -75 dBm → `requestNetwork` pre-warm di động), `LanLocks` (MulticastLock + WifiLock `FULL_LOW_LATENCY`), `UsbTetherDetector` (`rndis*/usb*/ncm*` qua `NetworkInterface`; USB tether = đường ưu tiên cao nhất). |
| `session/SessionController.kt` | Nối decoder, presenter, watchdog, ADPF, thermal, input cho một phiên. |
| `ui/` | `HomeScreen` (ID + danh sách LAN + nút QR giả), `SettingsScreen` (độ phân giải, FPS 30/60/90/120, chất lượng 1–100, codec, HW/SW…), `ModifierBar` (Win, Alt+Tab, Ctrl+Alt+Del, Esc, PrtSc), `RemoteSurfaceView`, `SessionScreen`. |

## Chế độ "Màn hình phụ (USB)" (kiểu spacedesk qua cáp) + âm thanh

Biến máy Android thành màn hình thứ hai của PC qua cáp USB; host tạo màn hình ảo khớp độ phân giải gốc của máy.

- **Đường chính: Android Open Accessory (AOA)** — `usb/AccessoryLink.kt`. Host Windows đưa điện thoại vào chế độ accessory
  (manufacturer `RemoteView`, model `RemoteViewHost`); Android phát `USB_ACCESSORY_ATTACHED`, app tự mở và vào phiên ("cắm là hiện").
  **Không cần bật tùy chọn nhà phát triển hay gỡ lỗi USB.** Bộ lọc: `res/xml/accessory_filter.xml`.
- **Dự phòng: ADB reverse** — `usb/AdbReverseLink.kt`. Host chạy `adb reverse tcp:47810 tcp:47810`, app kết nối `127.0.0.1:47810`.
  **Cần bật Gỡ lỗi USB (USB debugging).**
- Cả hai dùng cùng khung: `u32` big-endian độ dài + payload (tối đa 64 KiB) — `usb/DatagramLink.kt`. Kotlin chỉ chuyển byte;
  Rust (`nativeConnectLink` / `nativePushDatagram` / `onSendDatagramFromNative`) lo giao thức.
- Thứ tự ưu tiên (`usb/LinkSelector.kt`): USB accessory (đã cắm) → ADB reverse → USB tether/LAN → server.
- Chế độ hiển thị: toàn màn hình immersive, luôn sáng (`FLAG_KEEP_SCREEN_ON`), ẩn thanh phím (bật lại trong Cài đặt).
  Cảm ứng trực tiếp 1:1 (`input/DirectTouchMapper.kt`): tuyệt đối chuẩn hóa 0..65535, tối đa 10 điểm gửi dạng sự kiện **touch**
  (không phải chuột); S Pen gửi sự kiện **pen**. Hello caps gồm độ phân giải gốc + tần số quét cao nhất (ví dụ 2800x1752@120)
  để host tạo màn hình ảo tương ứng (`usb/DisplayCaps.kt`; tên trường JSON chưa chốt với giao thức).
- **Âm thanh đi kèm** (`audio/AudioPlayer.kt`): PCM 48 kHz stereo qua `AudioTrack` `PERFORMANCE_MODE_LOW_LATENCY`, bộ đệm nhỏ,
  jitter buffer ~30 ms (tối đa 40 ms, bỏ gói cũ nhất khi trễ). Opus giải mã bằng `MediaCodec` "audio/opus" (gói thô do lõi Rust chuyển lên; không tạo được codec thì gói Opus bị bỏ qua). Bật/tắt ở Cài đặt → "Âm thanh".
  Oboe/AAudio qua JNI là việc tương lai.

Chuỗi tiếng Việt: `app/src/main/res/values/strings.xml`.

## Chẩn đoán toàn diện (`diag/`, `ui/DiagnosticsScreen.kt`)

Nút "Chẩn đoán toàn diện" ở màn hình chính. Các mục chạy ở luồng nền, mỗi mục là một thẻ (thông tin / OK / cảnh báo / lỗi) kèm giải thích và cách xử lý bằng tiếng Việt; phần "Kết luận" ở đầu gộp kết quả.
Nên cắm cáp USB-C vào laptop đang bật trước khi chạy; cắm/rút cáp khi đang mở màn hình thì tự chạy lại (receiver `USB_STATE`).
Xuất báo cáo: "Sao chép báo cáo", "Chia sẻ", và tự lưu `diagnostics-<thời gian>.txt` trong thư mục file riêng của app (`getExternalFilesDir`; đường dẫn hiện trên màn hình).

| Mục | Phát hiện được | Không phát hiện được |
|---|---|---|
| Thiết bị | hãng, model (SM-X820 Wi-Fi / SM-X826 5G), Android, SoC (API 31+) | phiên bản One UI (không có API công khai, bỏ qua) |
| Mạng di động | FEATURE_TELEPHONY, phoneType (chỉ tham khảo) | menu tethering có bị ẩn hay không |
| Trạng thái USB | extra de-facto của `USB_STATE`: connected/configured/mtp/ptp/rndis/ncm/adb/midi/accessory | extra vắng mặt = "không rõ"; không phân biệt chắc cáp chỉ sạc với bộ sạc tường |
| USB accessory | FEATURE_USB_ACCESSORY/HOST, accessory RemoteView đã gắn, quyền | host chưa chạy thì không có accessory |
| Nhà phát triển/ADB | `DEVELOPMENT_SETTINGS_ENABLED`, `ADB_ENABLED` | adbd có được laptop cấp khóa RSA hay chưa |
| Giao diện mạng | mọi `NetworkInterface`, rndis/usb/ncm, transport, validated/metered, link speed Wi-Fi (không lấy SSID) | băng thông thực |
| Kết nối cục bộ | TCP 127.0.0.1:47810 (adb reverse đã thiết lập), UDP broadcast 47800 nghe 1,5 s | cổng mở chưa chắc host đang nghe; Wi-Fi chặn broadcast hoặc tường lửa khiến không thấy host |
| Ứng dụng | spacedesk có cài hay không (`<queries>`) | spacedesk có chạy được hay không |
| Bộ giải mã | AVC/HEVC/AV1/VP9: HW/SW, low-latency, hỗ trợ 2800x1752@120, số tham số vendor (API 31+) | hiệu năng thực tế; số liệu chỉ là khả năng báo cáo |
| Màn hình | mọi mode, tần số quét, HDR | |
| Nhiệt/pin | trạng thái nhiệt, headroom, tiết kiệm pin, pin, nguồn cắm | |
| Âm thanh | frames/buffer, sample rate, FEATURE_AUDIO_LOW_LATENCY | độ trễ thực |
| Theo dõi USB tethering | ghi 20 s mọi `USB_STATE` và giao diện rndis*/ncm*/usb* (300 ms), kết luận: thiếu driver Windows / Android từ chối / cáp chỉ sạc | cần người dùng tự bật công tắc trong 20 s |
| Thông tin Windows | các bước Device Manager (Remote NDIS / UsbNcm) | không đọc được trạng thái Windows |

## Việc còn thiếu / điểm cần xác minh trên máy thật

- Toàn bộ mã **chưa biên dịch**; chưa có bài kiểm thử.
- Lõi Rust (`rv_core`): mới có đường USB; chưa có LAN/server, mDNS. Lõi chưa chạy thử trên máy thật (chỉ có test đơn vị trên Linux).
- Dấu `tiltX/tiltY` của S Pen và hệ tọa độ pen phía Windows cần kiểm chứng.
- `ACCESS_LOCAL_NETWORK` (Android 16): tên quyền/cách hoạt động chưa xác minh với SDK 36.
- Telex trên bàn phím ảo: `setComposingRegion()` trên văn bản đã commit không được hỗ trợ (không giữ văn bản).
- Bậc ladder Exynos 2600 / MTK chỉ dựa trên tài liệu; khóa nào máy không báo hỗ trợ sẽ bị bỏ qua và ghi log (tag `RvDecoder`).
- Quét QR là chỗ giữ chỗ; kích thước giải mã ban đầu lấy theo màn hình (cần lấy từ host).
- Chưa có icon launcher, chưa cấu hình ký bản release.

## Kiểm tra giới hạn (Stress test)

Nút "Kiểm tra giới hạn" trên màn hình chính mở `bench/BenchScreen`. Không cần laptop: máy tự mã hóa một clip thử 2 s
bằng bộ mã hóa phần cứng (H.264/HEVC/AV1; không có B-frame, chỉ 1 IDR đầu clip, nội dung gồm nhiễu cuộn + vạch tương phản
+ bộ đếm khung), giữ trong RAM rồi lặp lại cho các bộ giải mã phần cứng (gồm biến thể `.lowlatency`) với khóa vendor của
`LowLatencyDecoder.buildTiers`. Đầu ra đi vào `ImageReader` ngoài màn hình.

- Chế độ: Throughput tối đa; Độ trễ ở FPS cố định (60/120/144, đo queueInput -> onOutputBufferAvailable); Bền nhiệt (120 fps, 1 hoặc 2 phút).
- 1–4 decoder song song, bật/tắt ADPF, bitrate clip 20–120 Mbps, độ phân giải native/2800x1752/2560x1440/1920x1080.
- Mỗi giây: fps, p50/p99 trễ, thermal headroom (10 s), thermal status, pin %, nhiệt pin, dòng CURRENT_NOW × điện áp = W ước tính, tần số quét.
- Tự dừng khi thermal status >= SEVERE (công tắc "Cho phép chạy tới CRITICAL" nâng ngưỡng dừng lên CRITICAL).
- Kết luận "bị bóp xung": fps giảm > 10% hoặc p99 tăng > 30% giữa 15 s đầu và 15 s cuối, kèm headroom >= 0.9 hoặc status >= MODERATE.
- Xuất: sao chép (báo cáo + CSV), chia sẻ (báo cáo), tự lưu `.txt`/`.csv` vào `getExternalFilesDir(null)/bench/`.

Chưa xác minh trên máy thật: mã chưa biên dịch; `getInputImage` + Image flexible trên encoder Exynos/MTK ở 2800x1752;
ImageReader PRIVATE làm đích của decoder; đơn vị CURRENT_NOW khác nhau theo hãng (đã đoán µA/mA); độ chính xác ADPF.

## Giao thức v2: codec và mất kết nối

- Hello quảng bá `"codecs"` (theo thứ tự ưu tiên): `hevc` khi có decoder HEVC phần cứng, `av1` chỉ khi bật AV1 trong cài đặt, `h264` luôn có; lựa chọn Auto/H.264/HEVC/AV1 lọc và sắp xếp danh sách (`DisplayCaps.advertisedCodecs`). Không còn ép H.264 ở chế độ USB.
- Lõi Rust báo `STATE_STREAM_CONFIG` (11, `"hevc 2800x1752@120 80000kbps"`): `SessionController` dựng lại `LowLatencyDecoder` với đúng mime/kích thước/fps khi có thay đổi.
- `STATE_DISCONNECTED` (4): hiện nhãn "Mất kết nối — đang kết nối lại…" (không thoát phiên); khi `STATE_ESTABLISHED` quay lại thì flush decoder và đợi keyframe. `STATE_FAILED` vẫn đưa về hộp thoại PIN.
