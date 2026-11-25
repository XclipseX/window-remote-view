# PROMPT GỬI GEMINI DEEP RESEARCH

> Copy toàn bộ phần dưới đây vào Gemini (chế độ Deep Research).

---

## VAI TRÒ
Bạn là một nhóm kỹ sư trưởng (Principal Engineers) chuyên về: video streaming thời gian thực, codec phần cứng (NVENC/AMF/QuickSync/MediaCodec), lập trình hệ thống Windows (DXGI/Direct3D11/Win32 input), Android native (NDK/MediaCodec/SurfaceView/Vulkan), mạng độ trễ thấp (UDP/QUIC/WebRTC/NAT traversal) và tối ưu năng lượng. Hãy nghiên cứu SÂU, có trích dẫn nguồn (docs chính thức, GitHub, paper, benchmark), ưu tiên thông tin mới nhất 2025–2026.

## MỤC TIÊU DỰ ÁN
Xây dựng app điều khiển máy tính từ xa (tương tự TeamViewer) nhưng:
- **100% miễn phí, mã nguồn mở**, không thu phí, không giới hạn phiên, không phụ thuộc server trả phí.
- **Chỉ 2 chức năng chính:**
  1. Cài được trên **Windows (10/11, x64 + ARM64)** và **Android (8.0+)**.
  2. **Android điều khiển Windows** dễ dàng (host = Windows, client = Android).
- **Tối ưu tối đa:** độ trễ end-to-end (mục tiêu < 30 ms LAN, < 80 ms Internet), điện năng (pin Android + CPU/GPU Windows), chất lượng hình ảnh, FPS (30/60/90/120/144).
- **Chỉ dùng codec mở/không bản quyền độc quyền ngốn tài nguyên:** ưu tiên H.264/H.265(HEVC)/AV1/VP9 qua API phần cứng; phân tích chi phí bản quyền/patent của từng codec với dự án miễn phí.
- **Hỗ trợ cả mã hóa/giải mã phần cứng VÀ phần mềm native** (fallback khi không có GPU phù hợp): ví dụ x264/OpenH264, libvpx, SVT-AV1/rav1e/dav1d.
- **Tùy chỉnh trong app:** độ phân giải, FPS, và **thanh trượt chất lượng hình ảnh 1–100%** (map sang bitrate/QP/CRF thế nào cho hợp lý — đề xuất công thức cụ thể).
- **Input đầy đủ:** bàn phím ảo + bàn phím vật lý/Bluetooth (kể cả phím đặc biệt: Ctrl+Alt+Del, Win, F1–F12, IME tiếng Việt/Unicode), touch (đa điểm, cử chỉ: chạm=click, 2 ngón=chuột phải/cuộn, pinch zoom), chế độ trackpad (rê chuột tương đối), chế độ chuột trực tiếp (tuyệt đối), chuột Bluetooth/OTG, bút stylus (áp lực nếu có).
- **Dễ dùng:** kết nối bằng ID + mật khẩu hoặc quét QR; tự tìm máy trong LAN.

## YÊU CẦU: CHIA AGENT XỬ LÝ
Hãy chia nghiên cứu thành các **Agent chuyên trách** dưới đây. Mỗi agent phải trả về: (a) phương án đề xuất, (b) so sánh tối thiểu 2–3 lựa chọn thay thế bằng bảng, (c) rủi ro & cách giảm thiểu, (d) code mẫu/pseudo-code hoặc API cụ thể, (e) nguồn tham khảo có link.

### Agent 1 — Kiến trúc tổng thể & tham khảo dự án mở
- Phân tích kiến trúc của: **RustDesk, Sunshine + Moonlight, Parsec (phần công khai), scrcpy, Apache Guacamole, Chrome Remote Desktop, KasmVNC, Selkies**.
- Rút ra điểm mạnh/yếu về độ trễ, CPU, pin; dự án nào có license cho phép fork/tái sử dụng (GPL/AGPL/MIT/Apache) và hệ quả pháp lý.
- Đề xuất: **tự viết mới** hay **fork/nhúng** (vd: fork Sunshine làm host + client Android riêng)? Đưa ra khuyến nghị rõ ràng.
- Chọn ngôn ngữ/stack: Rust vs C++ vs Kotlin/C++ NDK vs Flutter cho UI; vẽ sơ đồ module (capture → encode → transport → decode → render; input ngược chiều).

### Agent 2 — Capture màn hình Windows
- So sánh **DXGI Desktop Duplication**, **Windows.Graphics.Capture (WGC)**, GDI, NvFBC (đã bị hạn chế?).
- Zero-copy GPU: capture texture D3D11 → encoder phần cứng không qua RAM CPU.
- Xử lý: đa màn hình, HDR, DPI scaling, màn hình khóa/UAC/Secure Desktop (cần chạy như Windows Service + chuyển session), máy không có màn hình (virtual display driver/IddCx), con trỏ chuột vẽ riêng (gửi cursor shape riêng để giảm băng thông).
- Chỉ gửi khi có thay đổi (dirty rects) để tiết kiệm điện khi màn hình tĩnh.

### Agent 3 — Codec & Encoder (Windows)
- So sánh **NVENC (NVIDIA), AMF (AMD), Intel QSV/oneVPL, Media Foundation, D3D12 Video Encode** cho H.264 / HEVC / AV1.
- Cấu hình độ trễ thấp: preset ultra-low-latency, không B-frame, intra-refresh thay keyframe, VBV/HRD nhỏ, slice mode, CBR vs VBR, lookahead=0.
- Fallback phần mềm: x264 (tune zerolatency) / OpenH264 / SVT-AV1 — CPU cost bao nhiêu ở 1080p60.
- **Bản quyền/patent:** H.264, HEVC, AV1, VP9 — cái nào an toàn cho app miễn phí phân phối nhị phân (OpenH264 của Cisco, phần cứng đã trả license...).
- Đề xuất **công thức slider chất lượng 1–100%** → bitrate/QP theo độ phân giải và FPS; adaptive bitrate tự động theo mạng.

### Agent 4 — Decode & Render Android
- **MediaCodec** (NDK `AMediaCodec`) low-latency: `KEY_LOW_LATENCY`, `KEY_PRIORITY`, vendor-specific low-latency keys (Qualcomm, MediaTek, Exynos, Tensor), output thẳng vào `Surface`/`SurfaceView` (không qua TextureView).
- Kiểm tra codec hỗ trợ (`MediaCodecList`), chọn tự động H.264/HEVC/AV1 theo máy.
- Fallback phần mềm: **dav1d**, FFmpeg/libavcodec (h264), libvpx.
- Frame pacing (Choreographer, `setFrameRate`), màn hình 90/120Hz, tránh jitter buffer.
- Tiết kiệm pin: giải mã phần cứng, giảm FPS khi màn hình tĩnh, tắt khi app vào background, Wi-Fi low-latency lock (`WIFI_MODE_FULL_LOW_LATENCY`).

### Agent 5 — Mạng & truyền tải
- So sánh: **raw UDP + FEC tự viết (như Moonlight)**, **WebRTC**, **QUIC (quinn/msquic)**, **KCP**, **SRT**, TCP.
- NAT traversal miễn phí: STUN/TURN tự host (coturn), UDP hole punching, relay server tự host (như RustDesk hbbs/hbbr), Tailscale/WireGuard tích hợp?
- Discovery LAN: mDNS/Bonjour.
- Congestion control & ABR (GCC, BBR), FEC (Reed-Solomon), xử lý mất gói, ưu tiên kênh input (độ trễ input < video).
- Chi phí server signaling/relay tối thiểu — làm sao để **vận hành gần như 0 đồng** (P2P ưu tiên, relay fallback chạy VPS free tier).

### Agent 6 — Input injection (Windows) & UX điều khiển (Android)
- Windows: `SendInput`, ViGEm (gamepad), **Touch Injection API (`InjectSyntheticPointerInput`)**, pen injection, Unicode input, scan code vs virtual key, đặc quyền SYSTEM để gõ Ctrl+Alt+Del (SAS), đồng bộ clipboard.
- Android: thiết kế cử chỉ — chế độ Touch trực tiếp vs Trackpad, kéo thả, cuộn 2 ngón, zoom/pan, nhấn giữ = chuột phải, thanh phím đặc biệt nổi, hỗ trợ bàn phím/chuột Bluetooth và **pointer capture** (`requestPointerCapture`), IME tiếng Việt.
- Đo và tối ưu độ trễ input (gửi ngay, không gom theo frame; dự đoán con trỏ cục bộ – local cursor rendering).

### Agent 7 — Bảo mật
- Mã hóa đầu cuối (DTLS/TLS 1.3/Noise protocol), xác thực bằng mật khẩu một lần + khóa công khai, chống brute-force, xác nhận kết nối phía Windows, không lưu dữ liệu trên server.

### Agent 8 — Tối ưu điện năng & hiệu năng, đo đạc
- Phương pháp đo **độ trễ glass-to-glass** (camera 240fps, LED/photodiode), đo CPU/GPU/pin (Windows: PresentMon, GPUView, ETW; Android: Battery Historian, Perfetto, `dumpsys batterystats`).
- Bảng mục tiêu: bitrate/FPS/độ trễ/công suất cho các cấu hình 720p30, 1080p60, 1440p120, 4K60.
- So sánh số liệu thực tế với TeamViewer/AnyDesk/RustDesk/Parsec/Moonlight nếu có benchmark công khai.

### Agent 9 — Đóng gói, phân phối & UI
- Windows: installer (MSIX/WiX/Inno Setup), chạy dưới dạng Service, tự khởi động, ký code miễn phí (SignPath for OSS?), tránh bị Windows Defender/SmartScreen chặn.
- Android: phát hành APK trên GitHub Releases / F-Droid / Google Play (chính sách Accessibility/remote control).
- UI tối giản: màn hình chính chỉ có "ID của tôi / Kết nối tới", trang cài đặt có slider Độ phân giải / FPS / Chất lượng 1–100% / Codec (Tự động/H.264/HEVC/AV1) / Phần cứng–Phần mềm.
- CI/CD miễn phí bằng GitHub Actions.

### Agent 10 — Tổng hợp (Lead Architect)
Tổng hợp kết quả của 9 agent thành:
1. **Kiến trúc cuối cùng** (sơ đồ + lý do chọn).
2. **Tech stack cụ thể** (thư viện + phiên bản + license).
3. **Lộ trình MVP → v1.0** chia theo sprint 2 tuần, mỗi sprint có tiêu chí hoàn thành đo được.
4. **Cấu trúc thư mục repo** (monorepo: `host-windows/`, `client-android/`, `protocol/`, `relay-server/`, `docs/`).
5. **Định nghĩa giao thức** (định dạng gói video, input, control, cài đặt) dạng bảng hoặc protobuf/flatbuffers.
6. **Danh sách rủi ro lớn nhất** và cách xử lý.
7. Các **quyết định còn mở** cần người phát triển chọn.

## RÀNG BUỘC ĐẦU RA
- Viết bằng **tiếng Việt**, thuật ngữ kỹ thuật giữ tiếng Anh.
- Mọi khẳng định quan trọng phải có **nguồn trích dẫn**; ghi rõ khi thông tin chưa chắc chắn.
- Ưu tiên giải pháp **miễn phí, mã nguồn mở, ít tài nguyên**; không đề xuất SDK/codec trả phí.
- Có bảng so sánh, số liệu ước lượng, và code mẫu ngắn cho các phần then chốt (capture → NVENC, MediaCodec low-latency, SendInput/Touch injection, slider → bitrate).
