# Báo cáo nghiên cứu số 2 (Gemini) – Tối ưu thiết bị (Samsung + ASUS Vivobook) & kết nối LAN + Server

> Bản lưu tóm tắt các quyết định chính; nội dung đầy đủ do người dùng cung cấp trong phiên làm việc.
> Lưu ý: một số số liệu (thông số Exynos 2600, số issue Moonlight, độ trễ ước tính) là suy luận của Gemini, cần đo thực tế.

## A. Client Android
- S26+ (Exynos 2600): codec c2.exynos.* / c2.sec.*; HEVC/AV1 HW; KHÔNG có 4:4:4 HW. Đặt KEY_LOW_LATENCY=1, KEY_PRIORITY=0, KEY_OPERATING_RATE=Short.MAX_VALUE (chống tụt xung MFC).
- Tab S10+ (Dimensity 9300+): c2.mtk.*; vendor.rtc-ext-dec-low-latency.enable=1, vendor.low-latency.enable=1; KEY_OPERATING_RATE = fps (không vượt khai báo). Tránh H.264 trên MTK.
- Host bắt buộc: không B-frame, max_num_reorder_frames=0, bitstream_restriction_flag=1, max_dec_frame_buffering=1.
- SurfaceView + releaseOutputBuffer(idx, ts); setFrameRate(120, FIXED_SOURCE, CHANGE_FRAME_RATE_ALWAYS) + preferredDisplayModeId; appCategory="game"; ADPF target 6.5 ms.
- Thermal: headroom <0.75 full; 0.75–0.9 bitrate 70%; ≥0.9 → 60 FPS; SEVERE → 720p/1080p.
- Telex: BaseInputConnection, gửi commitText dạng Unicode; phím tắt gửi scancode. S Pen → InjectSyntheticPointerInput (PT_PEN).
- Mặc định: S26+ LAN HEVC10 2340×1080@120 50–80 Mbps; Tab S10+ LAN AV1/HEVC10 2800×1752@120 80–120 Mbps; Internet 1080p60 AV1 8–20 Mbps.

## A2. Host Vivobook S14 OLED (Ryzen AI 9 HX 370, Radeon 890M, VCN 4.0, Wi-Fi MT7922 6E)
- AMF HEVC/AV1 (không dùng H.264 vì chất lượng chữ kém); không 4:4:4.
- ULTRA_LOW_LATENCY, LOWLATENCY_MODE=true, pre-analysis off, VBV ≈ 1.5 frame, intra-refresh, in-flight ≤ 2.
- Zero-copy: DXGI → compute shader BGRA→NV12/P010 (async compute) → CreateSurfaceFromDX11Native.
- Static frame skip khi AccumulatedFrames==0; SetThreadExecutionState khi có phiên.
- IddCx virtual display khớp độ phân giải client, tắt màn OLED.
- NPU XDNA 2: KHÔNG dùng. ROI text bằng shader GPU.
- Tắt Wi-Fi background scan (WlanSetInterface) để tránh lag spike 60 s.
- Tự hạ cấu hình nếu encode > 7 ms.

## B. Kết nối LAN + Server
- Discovery 3 lớp: mDNS → UDP broadcast (MulticastLock) → quét subnet.
- Ghép đôi TOFU bằng PIN/QR; firewall rule cho cả Private + Public.
- Server: Oracle Cloud Always Free (10 TB egress) làm relay; GCP/Cloudflare chỉ hợp làm signaling.
- VN: ưu tiên IPv6 P2P (tăng tỷ lệ thành công trên 4G/5G); relay TCP/443 khi UDP bị chặn.
- Happy eyeballs: 0 ms LAN + signaling; 150–800 ms IPv6/IPv4 punch; 800 ms relay UDP; 1500 ms relay TCP/443; nâng cấp lên P2P ở nền.
- Gemini đề xuất QUIC (quinn) + datagrams cho migration — XUNG ĐỘT với báo cáo 1 (UDP + Noise). Xem quyết định trong README/nhật ký.
