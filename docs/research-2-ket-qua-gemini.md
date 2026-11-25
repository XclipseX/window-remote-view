# Báo cáo nghiên cứu số 2 (Gemini, bản làm lại) – Tóm tắt quyết định

> Bản này THAY THẾ bản tóm tắt trước. Số liệu Exynos 2600 là suy luận từ Exynos 2400/2500 — phải đo máy thật.

## A. Client Android
### Galaxy S26+ (Exynos 2600, Android 16, 3120×1440 LTPO 1–120 Hz, Wi-Fi 7)
- Codec dự kiến: c2.exynos.{h264,hevc,av1,vp9}.decoder. Không 4:4:4 HW.
- Khóa vendor (thang fallback, mỗi bậc MediaFormat mới):
  - bậc 0: `vendor.rtc-ext-dec-low-latency.enable=1`, `vendor.rtc-ext-dec-output-queue-depth.value=2`, `vendor.sec-dec-output.delay=0`
  - bậc 1–2: chỉ `vendor.rtc-ext-dec-low-latency.enable=1`
- KHÔNG đặt KEY_OPERATING_RATE trên Exynos (Moonlight ghi nhận crash/treo).
- Mặc định HEVC; chỉ bật AV1 khi đo < 8 ms @120fps.

### Galaxy Tab S10+ (Dimensity 9300+, 2800×1752 120 Hz, S Pen)
- Codec: c2.mtk.{avc,hevc,av1,vp9}.decoder (đã xác minh). Không 4:4:4 HW.
- C2 đã bỏ `vdec-lowlatency` → mặc định giữ 1–2 khung (15–32 ms).
- Thang: bậc 0 `KEY_OPERATING_RATE=fps` + `vendor.mtk-codec2.low-latency-mode=1` + `vendor.mtk-codec2.game-mode=1` + `vendor.low-latency.enable=1`; bậc 1 bỏ mtk-codec2.*; bậc 2 bỏ cả KEY_LOW_LATENCY.
- Gọi `getSupportedVendorParameters()` (API 31+) trước khi đặt khóa.
- AV1 trên D9300+ ~18–25 ms → KHÔNG dùng ở 120 Hz. Mặc định HEVC.
- Decoder Stall Watchdog: flush sau 1 s, tạo lại sau 5 s không có output.

### Chung
- Host: không B-frame, num_reorder_frames=0, max_dec_frame_buffering=1, pic_order_cnt_type=2 (H.264).
- SurfaceView; setFrameRate(fps, FIXED_SOURCE, CHANGE_FRAME_RATE_ALWAYS) + preferredDisplayModeId.
- Choreographer + Latest-Frame Rendering (bỏ khung cũ); tùy chọn Direct Present.
- ADPF target ~6–7 ms (120) / ~12 ms (60).
- `android:appCategory="productivity"` (KHÔNG "game" — tránh GOS bóp).
- Thermal: <0.75 full; 0.75–0.9 bitrate 70%; ≥0.9 → 60 fps.
- Telex: phím vật lý → scancode (Unikey/EVKey trên Windows xử lý); bàn phím ảo → BaseInputConnection gửi Unicode theo diff; chế độ Raw Typing.
- Phím bị One UI chặn: đọc getScanCode(), thanh phím nổi, key remapping.
- Android 16: xử lý Local Network Permission.
- Mặc định: S26+ LAN HEVC 2340×1080@120 60–80 Mbps; Tab S10+ LAN HEVC 2800×1752@120 80–120 Mbps; Internet HEVC 1080p/1200p@60 12–25 Mbps.

## A2. Host Vivobook S 14 OLED M5406WA (HX 370, Radeon 890M, VCN 4.0.5, 3K 120 Hz OLED, MT7922 Wi-Fi 6E)
- **Native AMF SDK**, không qua FFmpeg (benchmark RDNA3: giảm 44–46% độ trễ mã hóa).
- HEVC 8/10-bit mặc định; AV1 khi bitrate thấp; H.264 chỉ dự phòng. Không 4:4:4.
- USAGE ULTRA_LOW_LATENCY (fallback LOW_LATENCY), LOWLATENCY_MODE, B-frame 0, pre-analysis off, preset SPEED (120) / BALANCED (60).
- CBR + ENFORCE_HRD + FILLER_DATA off + VBV 1.5–2× khung; >60–80 Mbps bị overshoot → VBR_LATENCY peak 1.2×. KHÔNG dùng QVBR.
- IDR chỉ theo yêu cầu + periodic intra-refresh.
- Zero-copy: DXGI DD → compute shader/VideoProcessorBlt → NV12/P010 → CreateSurfaceFromDX11Native.
- Static-frame skip; ước tính 10–16 W toàn máy khi tắt OLED.
- SudoVDA / Virtual-Display-Driver (IddCx), chế độ exclusive tắt OLED; watchdog gỡ màn ảo.
- NPU: không dùng. ROI text bằng shader.
- Tránh lỗi AMF: Terminate() phiên dò trước khi tạo phiên mới; tắt Instant Replay/Overlay của Adrenalin.
- Wi-Fi: tắt background scan (WlanSetInterface), power saving off; khuyến nghị USB-C Ethernet.

## B. Kết nối LAN + Server
- Discovery: mDNS (DnsServiceRegister / NsdManager) → UDP broadcast mỗi 500 ms → quét /24 sau 2 s.
- Liệt kê mọi interface (hotspot, USB tethering, IPv6 link-local).
- Ghép đôi TOFU: Ed25519 + QR hoặc PIN qua SPAKE2+; lưu khóa ở DPAPI / Android Keystore.
- Firewall: `profile=any`. DSCP EF cho gói video.
- Server: OCI Always Free (tài khoản mới 2 OCPU/12 GB, 10 TB egress), region Singapore; GCP/AWS/Azure chỉ làm signaling; Cloudflare không relay UDP.
- VN: IPv6 di động 80–90%, FTTH 58–86% → ưu tiên IPv6 trước IPv4 50 ms.
- Happy eyeballs: t=0 LAN; t=150 ms P2P (timeout 1.5 s); t=500 ms làm ấm relay, dùng nếu P2P fail; nâng cấp ngầm lên P2P.
- Relay TCP/TLS 443 dự phòng; giới hạn ~25 Mbps/phiên; credential ngắn hạn; chặn dải IP riêng.
- Handover: Gemini khuyến nghị QUIC connection migration (QSwitch: 0.762 s vs 1.819 s WebRTC); kích hoạt chủ động qua NetworkCallback, làm ấm sẵn cellular.
