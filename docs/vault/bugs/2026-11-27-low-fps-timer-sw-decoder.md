---
date: 2026-11-27
tags: [bug, performance, fps, windows, android, amf, mf, sudovda, logging]
status: fixed-pending-hardware-test
---

# Low FPS (~58 FPS cap, 20.4 avg, 3.6 1% low) & Missing Host Log in Export ZIP (build `cf7ce8e`)

## Triệu chứng trên thiết bị thật
- Phần cứng: ASUS Vivobook S14 OLED (Ryzen AI 9 HX 370 / Radeon 890M VCN 4.0.5, Windows 11, SudoVDA) ↔ Galaxy Tab S10+ (`SM-X826B`, Dimensity 9300+, Android 17 API 37).
- Dù giảm độ phân giải và chất lượng xuống 40% (`120 FPS`), màn hình thống kê Windows ghi nhận:
  - `FPS hiện tại: 58`, `FPS trung bình: 20.4`, `FPS 1% thấp: 3.6`, `Thời gian khung: 49.0 ± 53.0 ms`, `Phương sai: 2806.42 ms²`, `Mã hóa: 16.9 ms`, `Bộ mã hóa: mf-hevc-hw`.
- Log Android cho thấy 6/7 lần chạy mở bộ giải mã phần mềm CPU `c2.android.hevc.decoder`, chỉ 1 lần mở `c2.mtk.hevc.decoder.lowlatency`.
- File ZIP xuất log từ Windows thiếu `remoteview-*.log` (chỉ có 2 file `remoteview-service-*.log` 500B).

## Nguyên nhân gốc rễ (Root Causes)
1. **Thiếu `timeBeginPeriod(1)` trên Windows (`crates/host-win/src/engine.rs`):** Độ phân giải timer mặc định của Windows là `15.625 ms` (64 Hz). Mọi lệnh `sleep`, `recv_timeout(2ms)` trong `run_session`, `cap.acquire(8)` trong `video_thread`, và chờ sự kiện MFT đều bị lượng tử hóa thành 15.6 ms → chặn trần ở ~58–64 FPS.
2. **Vòng lặp `run_session` chặn trên `io.recv(2ms)` (`engine.rs`):** Khi Android không gửi gói input, `run_session` ngủ 1 tick timer (15.6 ms) trước khi đọc `ctx.vrx` để gửi khung hình.
3. **Nhịp `video_thread` chia đôi 120 → 60 FPS (`engine.rs`):** `cap.acquire(8)` luôn chờ tối đa 8 ms rồi kiểm tra `elapsed + 0.5ms >= 8.33ms`; chỉ lệch 0.5 ms là bỏ qua nhịp và chờ thêm 8 ms nữa.
4. **AMF `QueryOutput` không poll `AMF_REPEAT` (`encode/amf.rs`) & `amfrt64.dll` chỉ tìm `SYSTEM32` (`encode/amf_ffi.rs`):** Nếu driver Radeon không block trong `QueryOutput` khi nhận `AMF_REPEAT`, `encode_frame` trả rỗng và `HwSupervisor` loại `amf-hevc-hw` sau 1 giây, rơi về `mf-hevc-hw`.
5. **Media Foundation `mf.rs` chưa đặt `CODECAPI_AVEncCommonQualityVsSpeed = 0` và bị reset sau `SetInputType`:** Khiến `mf-hevc-hw` mã hóa ở chế độ chậm (~16.9 ms).
6. **SudoVDA chưa ép chế độ 120 Hz bằng `ChangeDisplaySettingsExW` (`sudovda.rs`):** Windows có thể giữ màn hình ảo ở 60 Hz mặc định.
7. **Thống kê `frame_stats.rs` tính cả khoảng nghỉ tĩnh 1000 ms:** Khi màn hình đứng yên, keepalive 1 FPS bị gộp vào `mean` / `1% low` / `variance`, tạo ảo giác tụt FPS.
8. **Android `FramePresenter.kt` spam 360 message/giây lên main thread & `DisplayLock.kt` hạ LTPO xuống 10 Hz sau 500 ms**, cộng thêm `LowLatencyDecoder.kt` không ghi lỗi `tryTier` vào `RvLog` nên khó biết nguyên nhân rơi về `c2.android.hevc.decoder`.
9. **`logfile.rs::is_log_of` khớp nhầm `remoteview-service-*.log` với prefix `"remoteview"`:** 2 file log service mới hơn đẩy log GUI/host ra khỏi `export_set`.
10. **Thanh trượt chất lượng ở `apps/win-ui/src/app.rs` không debounce:** Kéo thanh trượt gửi 50 lệnh `SetQuality` / 2 giây, đồng thời `engine.rs` gửi kèm `DisplayConfigChange` mỗi lần đổi bitrate.

## Sửa đổi đã áp dụng
- `crates/host-win/src/engine.rs`: Thêm `HighResTimerGuard` (`timeBeginPeriod(1)`), `boost_thread_priority`, tách `io.try_recv()` + `vrx.recv_timeout(1ms)` trong `run_session`, tối ưu pacing `video_thread`, thêm `VideoEvent::Bitrate`, snap tỷ lệ gần 16:10 (`2800x1752 -> 2560x1600`), tăng `sync_channel(4)`.
- `crates/host-win/src/encode/{amf_ffi.rs,amf.rs,mf.rs}`: Fallback `LOAD_LIBRARY_SEARCH_DEFAULT_DIRS` + `GetLastError`, poll `QueryOutput` trên `AMF_REPEAT`, fallback `LOW_LATENCY`, đặt `CODECAPI_AVEncCommonQualityVsSpeed = 0` và áp lại sau `SetInputType`.
- `crates/host-win/src/sudovda.rs` & `capture_win.rs`: Thêm `ensure_display_mode` ép đúng `WxH@Hz` và log `@Hz` thực tế.
- `crates/host-win/src/frame_stats.rs`: Loại khoảng nghỉ tĩnh `> 500 ms` khỏi mean/var/1% low (đồng bộ với Android `FrameStats.kt`).
- `crates/host-win/src/logfile.rs`: Sửa `is_log_of` chỉ khớp chữ số sau `prefix-`, xuất riêng GUI / host / service / Android logs.
- `apps/win-ui/src/app.rs`: Thêm `QualityDebounce` (150 ms + flush khi thả kéo).
- `apps/android/`: Tối ưu `FramePresenter.kt` (idle watcher tự hẹn giờ, 3s), `DisplayLock.kt` (hạ 60 Hz thay vì 10 Hz), `LowLatencyDecoder.kt` + `SessionController.kt` (ghi lỗi từng tier vào `RvLog`, chặn spam `handle=0`), `DatagramLink.kt` (mở reader sau `onOpen`), `SettingsScreen.kt` (cảnh báo khi tắt giải mã phần cứng).
