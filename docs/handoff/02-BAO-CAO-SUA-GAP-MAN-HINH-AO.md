# Báo cáo bàn giao #2 — Nhánh sửa gấp `claude/urgent-virtual-display`

> Gửi Gemini (Antigravity). Đọc kèm `docs/handoff/01-BAO-CAO-BAN-GIAO.md` (bối cảnh toàn dự án) và vault `docs/vault/` (ADR-0003, ADR-0004, `sessions/2026-11-27.md`).
> **Trạng thái kiểm chứng: TẤT CẢ thay đổi dưới đây mới chỉ qua `cargo test`, `cargo clippy`, `cargo check --target x86_64-pc-windows-gnu` và CI GitHub Actions (build APK + RemoteView.exe xanh). CHỦ DỰ ÁN CHƯA CHẠY THỬ TRÊN MÁY THẬT.** Việc đầu tiên của bạn là đọc log thật khi chủ dự án gửi, không giả định các tính năng đã chạy.

## 1. Bối cảnh và lý do mở nhánh

- Laptop ASUS Vivobook S14 OLED (Ryzen AI 9 HX 370, Radeon 890M, Windows 11) **hỏng màn hình**. Trước đây chủ dự án dùng TeamViewer: TeamViewer tạo một màn ảo 1920×1080 (DISPLAY10), RemoteView chỉ *chụp lại* màn đó. Tắt TeamViewer → Windows không còn output nào hoạt động → RemoteView **đen hoàn toàn**.
- Gemini đang làm Đợt 1 (7 agent kiểm toán) thì hết quota, để lại `findings.md`. Chủ dự án yêu cầu mở nhánh riêng sửa gấp dựa trên báo cáo "Giải Pháp Kiến Trúc RemoteView" và `findings.md`.
- Nhánh: `claude/urgent-virtual-display`, tách từ `claude/remote-desktop-app-optimization-wsf4mm`. CI của nhánh này **cũng đăng lên Release `latest-build`** (link tải chung), nên bản tải hiện tại là của nhánh này.
- Phản hồi thật duy nhất đã có trong lúc làm: sau các bản đầu của nhánh, chủ dự án báo **"mượt hơn, 60 fps thẳng"**, nhưng khi thoát TeamViewer, Windows khóa máy và RemoteView không xem/điều khiển được màn hình khóa (TeamViewer thì làm được). Các bản sau đó (dịch vụ SYSTEM, UX, bộ đo FPS) **chưa được thử**.

## 2. Danh sách commit (cũ → mới)

| Commit | Nội dung |
|---|---|
| 26480b5 | RemoteView tự tạo màn hình ảo bằng SudoVDA |
| 3643b1d | Log DPI awareness + DPI hệ thống khi khởi động |
| 2dfeaa6 | Android: xả hết log upload trước khi đóng; ô long-press 64dp → 16dp |
| 43cc3a6 | USB writer thread gộp bulk; tái dùng staging texture; `--iid 0` cho 2D01; CI cài NASM |
| 12dda3c | Android: trạng thái native về main thread, decoder chờ StreamConfig, premultiply con trỏ, wake lock |
| c655922 | Host: chuỗi dự phòng encoder theo từng loại, validate VideoProcessor, OpenH264, con trỏ, pause |
| 45ffe4f | Vault |
| 1d1f70b | Tự cài driver khi mở app; mặc định "Màn hình ảo chính" |
| 86441f1 | Dịch vụ SYSTEM `RemoteViewSvc` cho màn hình khóa |
| 4944318 | Dịch vụ lỗi nhanh 2 lần → về tiến trình con |
| 877d002, dc54e4f | Android: HUD bộ đo FPS (bật mặc định) |
| b7df8e7 | Windows: tự chia sẻ, `--minimized`, preset chất lượng, bộ đo frame-time phía host |

## 3. Màn hình ảo SudoVDA (ADR-0003)

**Vấn đề gốc:** `vdisplay.rs::IddCxDisplay` là stub; engine chỉ chọn output có sẵn.

**Giải pháp:** driver IddCx **SudoVDA** (SudoMaker, MIT; bản DLL đã ký nằm trong installer Apollo).

- `crates/host-win/src/sudovda.rs` (mới). Hằng số lấy từ `Common/Include/sudovda-ioctl.h` thật (commit a4b09fa) và `third-party/sudovda/sudovda.h` của Apollo, **không phải mã minh hoạ trong báo cáo kiến trúc** (báo cáo dùng 0x801/0x802/0x803 sai):
  - interface GUID `{e5bcc234-1e0c-418a-a0d4-ef8b7501414d}`;
  - IOCTL `CTL_CODE(0x22, fn, 0, 0)` với ADD 0x800, REMOVE 0x801, GET_WATCHDOG 0x803, PING 0x888, GET_VERSION 0x8FF;
  - struct ADD 56 B `{u32 w, h, hz; GUID; char name[14]; char serial[14]}`, OUT `{LUID; u32 target}`.
- `SudoVdaMonitor::create(mode)`:
  1. mở thiết bị bằng SetupDi;
  2. REMOVE GUID cũ (nếu app crash trước đó);
  3. ADD với độ phân giải client (mặc định 2800×1752@120 của Tab S10+, hoặc `--width/--height`);
  4. luồng ping watchdog ≤1 s (driver tự gỡ màn sau 3 s nếu app chết);
  5. chờ tối đa 10 s cho QueryDisplayConfig ánh xạ (LUID, target) → `\\.\DISPLAYn`.
  - Drop thì REMOVE. GUID màn cố định để Windows nhớ scale/vị trí.
- `make_primary()`: ChangeDisplaySettingsEx(CDS_SET_PRIMARY | UPDATEREGISTRY | NORESET), dời các màn khác theo offset.
- `install(dir)`: certutil (root + TrustedPublisher) + SetupAPI tạo device node `Root\SudoMaker\SudoVDA` + `UpdateDriverForPlugAndPlayDevicesW` (tương đương install.bat/nefconc của Apollo).
- **engine.rs:**
  - Thêm `--display virtual`: tạo màn ảo và đặt làm chính.
  - `extend`: tạo màn ảo nhưng không đặt chính.
  - `mirror` mà **không có output nào** → tự tạo màn ảo và đặt làm chính.
- **CI** (`windows-app`):
  - Lấy release mới nhất của `ClassicOldSong/Apollo` qua `gh api`, giải nén installer (7z, hoặc `msiexec /a` nếu là .msi).
  - Lấy SudoVDA.dll/.inf/sudovda.cat/sudovda.cer, cache key `sudovda-apollo-v1`, nhúng bằng feature `embed-vdd`.
  - Log run 50 đã xác nhận "Embedding SudoVDA virtual display driver".
- **GUI:**
  - Tự cài khi mở app (`RemoteView.exe --install-all <dir>` qua UAC một lần, nhớ `vdd_auto_declined` nếu bị từ chối).
  - Có trạng thái + nút "Cài / cài lại".
  - Mặc định `Display::Virtual` cho cấu hình mới.

**Chưa kiểm chứng:** driver cài được trên Win11 24H2 hay không (Code 31/52?); màn ảo 2800×1752 có lên không; DXGI có duplicate được không; make_primary có đúng không. Xem log dòng `sudovda:` / `vdisplay:`.

## 4. Xử lý `findings.md` của Gemini

Mỗi mục đều được agent con kiểm chứng trong code trước khi sửa. Kết luận:

**Đúng, đã sửa:**
1. **Android "bắt tay xong nhưng không có hình":**
   - Nguyên nhân: `MainActivity` gọi `onNativeState` trên luồng Rust "rv-session" (không có Looper) → `FramePresenter.stop()` gọi `Choreographer.getInstance()` → IllegalStateException → `jni_bridge.rs` xoá exception im lặng → decoder cũ (AV1) chạy mãi.
   - Sửa: `onNativeState` post về main thread (giữ thứ tự, cờ `released`); `FramePresenter.stop()` an toàn với luồng; JNI log tên callback khi có exception.
   - Decoder chỉ dựng khi có StreamConfig. Nếu sau ESTABLISHED 1,5 s vẫn chưa có (host cũ) thì dựng decoder mặc định.
   - Cờ `videoStarted` để cấu hình giống cũ vẫn dựng lại nếu lần trước hỏng.
   - AV1 không quảng bá cho decoder MTK trừ khi người dùng chọn rõ.
2. **Ô 64dp góc trên-phải nuốt nút [X]:** giảm còn 16dp.
3. **Log Android bị cắt khi Shutdown:** worker giờ xả hàng đợi LogChunk tối đa 2,5 s trước khi thoát; giới hạn upload 2 MiB.
4. **USB ~135 bulk write đồng bộ/khung trên luồng session:**
   - `crates/usb-link/src/link.rs` có luồng `rv-usb-write` gộp bản ghi tới ~256 KiB mỗi write, hàng đợi 1024 (backpressure).
   - Định dạng dây giữ nguyên (u32 BE length-prefix); Android đọc theo luồng nên tương thích.
   - Có test.
5. **Staging texture 8,3 MB tạo lại mỗi khung:** `StagingCache` theo (w, h, format).
6. **`--iid 0`:** chỉ thêm cho 18D1:2D01 (composite). 2D00 là một interface, thêm vào sẽ sai.
7. **OpenH264 thiếu NASM:** CI cài NASM bằng choco, rust-cache key đổi thành `windows-app-nasm`.
8. **AMD MFT chết ở khung 0 + chặn cả hai encoder:**
   - Lỗi bị nuốt và đổi D3D manager giữa luồng: nay validate VideoProcessor (input/output view + Blt thử) **trước** khi đặt D3D manager/START_OF_STREAM; lỗi thì log HRESULT và init thất bại; không bao giờ đổi manager giữa luồng.
   - Copy texture dùng SHADER_RESOURCE|RENDER_TARGET. Khẳng định "chỉ SHADER_RESOURCE thì input view fail" **chưa được chứng minh**, chỉ mở rộng cờ cho an toàn.
   - NV12 thêm BIND_VIDEO_ENCODER (có dự phòng). `ctx.Flush()` sau Blt.
   - Chuỗi dự phòng mới, NO_D3D theo từng loại: HEVC D3D → H.264 D3D → HEVC sysmem → H.264 sysmem → OpenH264.
9. **OpenH264 chậm/mờ:**
   - skip_frames bật, QP 0..40, complexity Low, `bgra_to_i420` chạy song song theo băng (giữ BT.709).
   - Tự hạ xuống ~1280×800 (giữ tỉ lệ) nếu encode trung bình > 1000/fps ms trong 3 s; chỉ hạ, không tự nâng.
   - **Hạn chế:** crate openh264 0.9.10 cố định SM_SINGLE_SLICE nên `num_threads` gần như vô tác dụng; muốn đa luồng thật phải dùng API thô.
10. **`last_encode` đóng dấu sau encode:** đổi sang mốc trước encode.
11. **Con trỏ:**
    - Gửi lại shape + vị trí khi phiên mới / RequestIdr / hết pause.
    - Android premultiply alpha (hết viền trắng).
    - Scale shape/hotspot theo `GetDpiForMonitor / GetDpiForSystem`. **Rủi ro scale đôi** vì tiến trình là PerMonitorV2; nếu con trỏ to/nhỏ bất thường, cho `monitor_dpi_scale` trả 1.0.
12. **Pause:** audio và cursor không gửi khi video tạm dừng.
13. **Injector dùng Rect lúc khởi động:** nay cập nhật theo `sh.cap_rect`. Công thức chuẩn hoá đổi sang nghịch đảo chính xác `ceil(px*65536/size)`, có test round-trip mọi pixel.
14. **Wake lock:** PARTIAL_WAKE_LOCK trong phiên; AudioPlayer dừng ở onStop, mở lại ở onStart.

**Không đúng / không cần sửa:**
- "Slider bị bỏ qua ở USB": host áp dụng mọi link như nhau. Android không có slider trong phiên. `nativeSetBitrate`/`nativeSetMaxFps` là stub ở **mọi** chế độ; đây là thiếu tính năng, cần thêm message giao thức.
- "Con trỏ lệch do DPI awareness": host đã gọi PER_MONITOR_AWARE_V2. Đã thêm log `dpi: set_per_monitor_v2=… awareness=… system_dpi=…` để xác nhận bằng log thật.
- Bitmap con trỏ mất khi Surface tạo lại: không đúng, `_cursor` là StateFlow riêng.

## 5. Dịch vụ SYSTEM cho màn hình khóa (ADR-0004)

**Yêu cầu:** giống TeamViewer, xem/điều khiển cả khi Windows khóa (desktop Winlogon). Chỉ tiến trình LocalSystem trong phiên console mới OpenInputDesktop + SetThreadDesktop + DXGI duplicate + SendInput được trên Winlogon.

- **`RemoteView.exe --service`** (`apps/win-ui/src/service.rs`; phần thuần ở `crates/host-win/src/service_proto.rs`, có test):
  - Dịch vụ `RemoteViewSvc` (auto start, LocalSystem), viết trực tiếp bằng crate `windows`, không thêm crate.
  - Named pipe `\\.\pipe\RemoteViewSvc`, SDDL `D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)`, một client, `PIPE_REJECT_REMOTE_CLIENTS`.
  - Dòng đầu từ client là `{"op":"spawn","args":[...]}`, qua allow-list cờ.
  - Dịch vụ chạy **chính exe của nó** với `--host-worker`:
    - DuplicateTokenEx(self) → SetTokenInformation(TokenSessionId = WTSGetActiveConsoleSessionId) → CreateProcessAsUserW(`winsta0\default`), stdio qua anonymous pipe;
    - env block của user (WTSQueryUserToken) để dùng đúng %APPDATA%/usb_token;
    - relay dòng hai chiều; client ngắt thì gửi stop, chờ 3 s rồi kill.
- **Cài đặt:**
  - `--install-all <dir|->` cài SudoVDA và tạo/cập nhật dịch vụ (ChangeServiceConfig nếu đã có).
  - `--uninstall-service` để gỡ.
  - GUI tự chạy lúc mở app nếu thiếu (một UAC).
- **GUI transport** (`host.rs`):
  - Nếu pipe kết nối được thì dùng pipe, ngược lại dùng tiến trình con như cũ.
  - **Nếu dịch vụ chết nhanh (<15 s) 2 lần liên tiếp, app chuyển về tiến trình con cho phần còn lại của lần chạy.**
  - Log ghi rõ transport: `host: dịch vụ RemoteView (SYSTEM) qua pipe…` hoặc `host: tiến trình con…`.
- **Worker bám desktop** (`session_state.rs`: `running_as_system`, `follow_input_desktop`, `DesktopFollower`):
  - Capture poll 250 ms, đổi desktop thì mở lại DXGI/GDI. Cursor 100 ms. Session loop 50 ms trước khi inject.
  - Khi là SYSTEM, `sh.locked` luôn false nên không chặn capture/input. Log `desktop: Default -> Winlogon`.
- **Rủi ro, chưa kiểm chứng:**
  - Toàn bộ đường CreateProcessAsUser.
  - DXGI trên Winlogon (tuỳ driver GPU).
  - Touch/pen qua `InjectSyntheticPointerInput` có thể không tới Winlogon (chuột/phím SendInput khả thi hơn).
  - Self-update không tự cài lại dịch vụ (binPath trỏ exe cũ nếu đổi chỗ).
  - Bảo mật: mọi user tương tác mở được pipe; binPath ở thư mục user ghi được thì có thể leo quyền (nên đặt exe ở Program Files).
  - Ở màn đăng nhập không có user nên token USB có thể khác.

## 6. UX "ít thao tác nhất" + bộ đo FPS

**Windows** (`apps/win-ui`):
- `auto_share` (mặc định true): mở app → cài driver/dịch vụ nếu thiếu (UAC một lần) → tự bắt đầu chia sẻ.
- `--minimized` (registry autostart đã ghi sẵn) → thu nhỏ ở khung đầu + tự chia sẻ. eframe 0.32 không có `with_minimized`; dùng `ViewportCommand::Minimized`, có thể nháy.
- Home có ô **Chất lượng** với 3 preset (Tự động 120 fps/Q85, Mượt nhất 120/Q70, Nét nhất 60/Q100; hiện "Tùy chỉnh" khi không khớp) và ô "Tự chạy cùng Windows". Thiết lập chi tiết nằm trong "Nâng cao" (mặc định đóng).
- Mặc định Config mới: 120 fps, Q85.
- Giới hạn: chọn lại "Tự động" khi đang chạy không đưa độ phân giải về "theo thiết bị" (giao thức thiếu lệnh reset).

**Bộ đo phía host** (`crates/host-win/src/frame_stats.rs`, có 6 test):
- Đo khoảng cách giữa các khung đã **gửi** sau encode, cửa sổ 10 s / 1200 mẫu.
- `stats` JSON thêm `fps_now, fps_avg, fps_low1, frame_ms_mean, frame_ms_std, frame_ms_var` (tài liệu ở CONTROL_PROTOCOL.md).
- GUI tô màu 1% low: xanh ≥90% mục tiêu, vàng ≥60%, đỏ thấp hơn. Mục tiêu chặn 60 khi dùng OpenH264.

**Android** (`render/FrameStats.kt` + HUD):
- Đo lúc khung được present (CHOREOGRAPHER dùng frameTimeNanos; DIRECT dùng nanoTime sau releaseAt). Cửa sổ 10 s, bỏ khoảng >500 ms.
- HUD góc trên-trái, không nhận chạm, cập nhật 500 ms, **bật mặc định**: `FPS · Avg · 1% · mean±σ ms · var` + decoder/độ phân giải/khung bỏ.
- Chất lượng mặc định 85. Immersive cho mọi phiên.
- Luồng cắm USB đã đi thẳng vào phiên, không cần sửa.
- Thêm junit + `FrameStatsTest` nhưng CI chỉ chạy `assembleDebug` nên **test Kotlin chưa chạy**.

## 7. Việc còn lại (đề xuất cho Gemini)

**P0 — chờ log thật:**
1. Đọc zip "Xuất log" mới. Kiểm theo thứ tự:
   - `sudovda:` / `vdisplay:` (màn ảo lên chưa, có đặt chính không);
   - `host:` (dùng pipe hay tiến trình con);
   - `service:` / `desktop:` (Winlogon chụp được không);
   - `video:` (encoder nào thắng, có còn NOTACCEPTING không);
   - `dpi:` (con trỏ có scale đôi không);
   - stats `fps_low1`.
2. Nếu SudoVDA lỗi cài: xem Device Manager / `setupapi.dev.log`; cân nhắc dùng `nefconc.exe` như Apollo.
3. Nếu dịch vụ lỗi: tạm tắt bằng `--uninstall-service`, app tự quay về tiến trình con.

**P1:**
- Lệnh giao thức đổi bitrate/fps trong phiên (thay stub `nativeSetBitrate`).
- Reset độ phân giải "theo thiết bị".
- AMF native nếu MF vẫn hỏng.
- OpenH264 đa slice qua API thô.
- Ctrl+Alt+Del bằng SendSAS từ dịch vụ.
- Dịch vụ tự cập nhật binPath sau self-update.

**P2 (từ findings, chưa đụng):**
- Cổng LAN 47800/47900 lệch; `nativeConnect`/discovery là stub.
- Ping/RTT + `decode_ms` HUD.
- Tray icon; manifest PerMonitorV2 + icon PE; SHA-256 cho updater.
- Telex KeyboardBridge; pinch-zoom SurfaceView; Job Object cho worker.
- Tách CI + chạy `cargo test` trên Windows; ký số (SignPath).

**Quy tắc:**
- Luôn phát hành APK + exe cùng commit.
- Chạy test/clippy/cross-check trước khi push.
- Ghi vault (`sessions/`, `decisions/`, `bugs/`).
- Gộp nhánh này về nhánh chính khi chủ dự án xác nhận chạy được.
