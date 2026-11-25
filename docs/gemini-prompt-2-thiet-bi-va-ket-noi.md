# PROMPT NGHIÊN CỨU SỐ 2 – TỐI ƯU THIẾT BỊ (SAMSUNG + ASUS VIVOBOOK) & KẾT NỐI LAN + SERVER

## 1. Vai trò và bối cảnh
Bạn là chuyên gia về Android multimedia (MediaCodec, Codec2, vendor HAL), chip di động Samsung Exynos và MediaTek Dimensity, cùng mạng thời gian thực (UDP, QUIC, NAT traversal). Tôi đang phát triển một ứng dụng điều khiển máy tính từ xa mã nguồn mở, miễn phí: máy Windows phát hình (host), thiết bị Android điều khiển (client). Ứng dụng dùng codec mở (H.264, HEVC, AV1) với mã hóa/giải mã phần cứng, có phần mềm làm dự phòng. Kiến trúc tổng thể đang được nghiên cứu ở một phiên khác. Phiên này CHỈ tập trung vào hai chủ đề dưới đây, cần đi thật sâu, có trích dẫn nguồn (tài liệu Android, mã nguồn AOSP, GitHub issue của Moonlight, scrcpy, Sunshine, RustDesk, Parsec, Steam Link, diễn đàn XDA, benchmark) và ưu tiên thông tin năm 2025–2026. Thông tin nào chưa kiểm chứng được thì ghi rõ mức độ chắc chắn.

## 2. Chủ đề A – Tối ưu riêng cho hai thiết bị mục tiêu

### Thiết bị
- **Samsung Galaxy S26+ dùng Exynos 2600** (GPU Xclipse, One UI, Android 16).
- **Samsung Galaxy Tab S10+ dùng MediaTek Dimensity 9300+** (GPU Immortalis-G720, màn hình 12.4 inch 2800×1752, 120 Hz, có S Pen).

Nếu dữ liệu công khai về Exynos 2600 còn ít, hãy suy luận từ Exynos 2400 và 2500 (cùng dòng GPU Xclipse, cùng bộ codec của Samsung) và nói rõ đó là suy luận.

### Câu hỏi cần trả lời cho TỪNG thiết bị
1. **Khả năng giải mã phần cứng:** hỗ trợ H.264, HEVC Main/Main10, AV1, VP9 ở độ phân giải và FPS tối đa bao nhiêu; có giải mã YUV 4:4:4 không. Tên codec cụ thể trong MediaCodecList (ví dụ c2.exynos.*, c2.mtk.*), cách liệt kê bằng adb (`dumpsys media.player`, `/vendor/etc/media_codecs*.xml`).
2. **Chế độ độ trễ thấp:** `KEY_LOW_LATENCY` có thực sự được tôn trọng không; các khóa vendor riêng (ví dụ `vendor.rtc-ext-dec-low-latency.enable` của MediaTek, các khóa tương ứng của Samsung/Exynos); cách Moonlight-Android xử lý từng hãng và những lỗi đã biết khiến bộ giải mã giữ lại 1–3 khung hình.
3. **Độ trễ giải mã ước tính** (ms/khung) ở 1080p60, 1440p120 và 2800×1752@120; so sánh giữa H.264, HEVC, AV1.
4. **Hiển thị:** dùng SurfaceView với `setFrameRate`, đồng bộ Choreographer, ADPF (Android Dynamic Performance Framework) / PerformanceHintManager để giữ xung nhịp ổn định; cách khóa 120 Hz khi đang stream mà không bị One UI hạ về 60 Hz.
5. **Pin và nhiệt:** ảnh hưởng của Game Optimizing Service, chế độ tiết kiệm pin, Thermal API (`getThermalHeadroom`); chiến lược giảm FPS/bitrate động khi máy nóng; điện năng tiêu thụ ước tính trong 1 giờ stream.
6. **Nhập liệu đặc thù Samsung:** S Pen (áp lực, độ nghiêng, nút bên, hover) truyền sang Windows Pen Injection; Samsung DeX (chế độ cửa sổ, chuột/bàn phím ngoài); Book Cover Keyboard và phím tắt bị One UI chặn (phím Samsung DeX, phím tìm kiếm); `requestPointerCapture` cho chuột Bluetooth; bàn phím Samsung Keyboard với tiếng Việt Telex.
7. **Cấu hình mặc định đề xuất** cho từng máy: codec, độ phân giải, FPS, bitrate, khi dùng LAN và khi qua Internet; kèm cấu hình “tiết kiệm pin”.
8. **Kế hoạch kiểm thử** trên hai máy: công cụ đo (Perfetto, `dumpsys gfxinfo`, đo độ trễ bằng camera quay chậm), kịch bản thử, chỉ số chấp nhận.

## 2b. Chủ đề A2 – Tối ưu máy Windows phát hình (host): ASUS Vivobook S14 OLED, Ryzen AI 9 HX 370

### Thiết bị
- **ASUS Vivobook S14 OLED** dùng **AMD Ryzen AI 9 HX 370** (Zen 5 + Zen 5c, iGPU Radeon 890M kiến trúc RDNA 3.5, bộ mã hóa video VCN, NPU XDNA 2), màn hình OLED độ phân giải cao, tần số quét cao, Windows 11. Hãy xác minh chính xác thông số màn hình (độ phân giải, Hz, HDR) và phiên bản VCN.

### Câu hỏi cần trả lời
1. **Mã hóa phần cứng qua AMD AMF:** VCN trên Radeon 890M mã hóa được H.264, HEVC, AV1 ở độ phân giải/FPS tối đa bao nhiêu; số phiên mã hóa đồng thời; có hỗ trợ HEVC/AV1 YUV 4:4:4 không. So sánh chất lượng AMF với NVENC/QuickSync ở bitrate thấp, đặc biệt nội dung chữ văn phòng (AMF H.264 từng bị đánh giá kém — đến 2025–2026 đã cải thiện chưa?).
2. **Cấu hình độ trễ thấp AMF:** usage ULTRA_LOW_LATENCY / LOW_LATENCY, tắt B-frame, intra-refresh, `AMF_VIDEO_ENCODER_LOWLATENCY_MODE`, rate control CBR/VBR/QVBR, pre-analysis, kích thước VBV; độ trễ mã hóa ước tính (ms/khung) ở 1080p60, 1440p120, độ phân giải gốc màn OLED.
3. **Zero-copy pipeline:** DXGI Desktop Duplication hoặc Windows.Graphics.Capture → texture D3D11 → AMF trực tiếp, không copy về CPU; chuyển đổi màu BGRA→NV12 trên GPU; cách Sunshine và RustDesk làm với AMD.
4. **Điện năng và nhiệt:** đây là laptop mỏng nhẹ, cần tối ưu pin khi làm host. So sánh tải CPU/GPU và công suất (W) giữa mã hóa phần cứng AMF và phần mềm (x264, SVT-AV1 trên Zen 5); ảnh hưởng của chế độ nguồn Windows, ASUS MyASUS (Whisper/Standard/Performance), AMD SmartShift, khi cắm sạc và khi chạy pin. Giảm FPS/không mã hóa khi màn hình tĩnh.
5. **Màn hình OLED và HDR:** chụp nội dung HDR/10-bit, tone mapping sang SDR cho điện thoại, xử lý tần số quét cao và VRR; hiện tượng chống burn-in của OLED (dịch điểm ảnh, tự tắt màn) ảnh hưởng gì khi stream; tắt màn hình laptop khi đang được điều khiển để tiết kiệm điện mà vẫn chụp được hình (virtual display IddCx).
6. **NPU XDNA 2 (≈50 TOPS):** có đáng dùng NPU cho việc gì không — ví dụ upscale/khử nhiễu, phát hiện vùng chữ để tăng chất lượng cục bộ (ROI encoding), dự đoán chuyển động con trỏ? Nếu không đáng, nói rõ lý do.
7. **Driver và lỗi đã biết:** phiên bản driver AMD Adrenaline khuyến nghị, lỗi AMF trên RDNA 3.5 được ghi nhận trong GitHub issue của Sunshine/OBS/FFmpeg/RustDesk, cách khắc phục.
8. **Mạng phía host:** card Wi-Fi của máy (Wi-Fi 6E/7?), tắt power-save Wi-Fi, cấu hình ưu tiên khi dùng LAN với S26+ và Tab S10+.
9. **Cấu hình đề xuất** cho ba cặp: Vivobook → S26+, Vivobook → Tab S10+ (độ phân giải gốc 2800×1752@120), và chế độ tiết kiệm pin khi laptop chạy pin; gồm codec, độ phân giải, FPS, bitrate, chế độ nguồn.

## 3. Chủ đề B – Hỗ trợ đồng thời kết nối LAN và qua Server

Ứng dụng phải có CẢ HAI chế độ, tự động chọn đường tốt nhất, người dùng không phải cấu hình gì.

### B1. Chế độ LAN (không cần Internet)
- Tự phát hiện máy Windows: so sánh mDNS/DNS-SD (NsdManager trên Android, Bonjour/dns-sd trên Windows), UDP broadcast/multicast, quét subnet. Xử lý router chặn multicast, Android chặn multicast khi chưa giữ `MulticastLock`.
- Ghép đôi lần đầu bằng mã PIN hoặc QR, lưu khóa tin cậy (trust-on-first-use), lần sau tự kết nối.
- Hoạt động khi mất Internet: cùng Wi-Fi, hotspot điện thoại, Wi-Fi Direct, cáp USB tethering, mạng LAN dây + Wi-Fi cùng router.
- Tối ưu cho LAN: bitrate 50–150 Mbps, FEC thấp, bộ đệm jitter gần 0, Wi-Fi 6/6E, `WIFI_MODE_FULL_LOW_LATENCY`, tắt power-save Wi-Fi khi stream.
- Tường lửa Windows: tự tạo rule khi cài đặt, xử lý mạng được đánh dấu “Public”.

### B2. Chế độ Server (qua Internet)
- Máy chủ tự host miễn phí: ID/signaling server, STUN, relay (coturn TURN hoặc mô hình hbbs/hbbr của RustDesk). So sánh độ phức tạp, tài nguyên, băng thông.
- Triển khai bằng Docker Compose trên VPS free tier (Oracle Cloud Always Free, Google Cloud e2-micro…), máy nhà, hoặc Cloudflare (giới hạn với UDP?). Ước tính chi phí và giới hạn băng thông.
- Ưu tiên P2P: UDP hole punching, ICE, IPv6 trực tiếp; tỷ lệ thành công khi gặp CGNAT của Viettel, VNPT, FPT và mạng di động Việt Nam; khi nào bắt buộc relay.
- Phương án thay thế: tích hợp WireGuard/Tailscale/Headscale hoặc ZeroTier tự host — ưu nhược điểm so với tự viết.
- Bảo mật: mã hóa đầu cuối để relay không đọc được nội dung; xác thực ID + mật khẩu; chống dò quét.

### B3. Chuyển đổi và lựa chọn đường đi
- Thứ tự thử song song khi kết nối: LAN trực tiếp → P2P qua Internet → relay; thời gian chờ tối đa mỗi bước; cơ chế “happy eyeballs”.
- Chuyển mạng liền mạch khi điện thoại rời Wi-Fi sang 4G/5G hoặc ngược lại: QUIC connection migration, ICE restart, hoặc kết nối lại nhanh giữ nguyên phiên; mục tiêu gián đoạn dưới 1 giây.
- Tự điều chỉnh cấu hình (bitrate, FEC, FPS) theo loại đường đi đang dùng.
- Bảng so sánh độ trễ, băng thông, chi phí, độ ổn định của LAN, P2P và relay.

## 4. Yêu cầu đầu ra
- Viết bằng tiếng Việt, giữ thuật ngữ kỹ thuật tiếng Anh.
- Mỗi chủ đề có: khuyến nghị cuối cùng, bảng so sánh, rủi ro và cách giảm thiểu, mã mẫu ngắn (Kotlin/C++ NDK cho cấu hình MediaCodec theo hãng; C++ cho pipeline D3D11 → AMF độ trễ thấp; Rust/C++ cho discovery LAN và hole punching; file `docker-compose.yml` mẫu cho server).
- Cuối bài: danh sách việc cần làm theo thứ tự ưu tiên và các câu hỏi còn bỏ ngỏ cần thử nghiệm thực tế trên S26+, Tab S10+ và Vivobook S14 OLED.
