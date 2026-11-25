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
2. Lõi Rust (`librv_core.so`) chưa có: đặt bản build `aarch64-linux-android` (cargo-ndk) vào
   `app/jniLibs/arm64-v8a/`. Thiếu thư viện thì `NativeBridge.available == false` và UI vẫn chạy, lệnh native là no-op.

## Cấu trúc (`app/src/main/java/com/remoteview/client/`)

| Đường dẫn | Vai trò |
|---|---|
| `MainActivity.kt` | Điều hướng Home / Cài đặt / Phiên; xin quyền; lock Wi-Fi theo vòng đời; PauseVideo ở `onStop`. |
| `NativeBridge.kt` | Khai báo JNI tới Rust (`external fun`), callback ngược, `NativeInputSink`. Chỉ là giao diện + TODO. |
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

Chuỗi tiếng Việt: `app/src/main/res/values/strings.xml`.

## Việc còn thiếu / điểm cần xác minh trên máy thật

- Toàn bộ mã **chưa biên dịch**; chưa có bài kiểm thử.
- Lõi Rust (`rv_core`): kết nối, giải mã gói, mDNS, mã hóa, gửi input. Hiện chỉ là khai báo JNI.
- Dấu `tiltX/tiltY` của S Pen và hệ tọa độ pen phía Windows cần kiểm chứng.
- `ACCESS_LOCAL_NETWORK` (Android 16): tên quyền/cách hoạt động chưa xác minh với SDK 36.
- Telex trên bàn phím ảo: `setComposingRegion()` trên văn bản đã commit không được hỗ trợ (không giữ văn bản).
- Bậc ladder Exynos 2600 / MTK chỉ dựa trên tài liệu; khóa nào máy không báo hỗ trợ sẽ bị bỏ qua và ghi log (tag `RvDecoder`).
- Quét QR là chỗ giữ chỗ; kích thước giải mã ban đầu lấy theo màn hình (cần lấy từ host).
- Chưa có icon launcher, chưa cấu hình ký bản release.
