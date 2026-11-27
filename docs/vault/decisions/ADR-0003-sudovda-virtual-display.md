---
tags: [adr, host-win, display]
date: 2026-11-27
status: accepted
---
# ADR-0003: RemoteView tự tạo màn hình ảo bằng SudoVDA

## Bối cảnh
Màn laptop của chủ dự án bị hỏng. Không có TeamViewer (màn ảo DISPLAY10) thì Windows không có output nào đang hoạt động → DXGI/GDI chụp ra **màn đen**. Engine cũ chỉ *chọn* output có sẵn (`ExistingOutput`), `IddCxDisplay` là stub.

## Quyết định
- Dùng driver IddCx **SudoVDA** (SudoMaker, MIT; bản đã ký nằm trong installer Apollo). CI (`windows-app`) tải release mới nhất của `ClassicOldSong/Apollo`, giải nén (7z / msiexec /a), lấy `SudoVDA.dll/.inf/sudovda.cat/sudovda.cer`, nhúng bằng feature `embed-vdd`.
- Cài: `RemoteView.exe --install-vdd <dir>` chạy quyền admin (UAC) = certutil (root + TrustedPublisher) + SetupAPI tạo device node `Root\SudoMaker\SudoVDA` + `UpdateDriverForPlugAndPlayDevicesW` (tương đương install.bat/nefconc của Apollo).
- Dùng: `crates/host-win/src/sudovda.rs` mở interface `{e5bcc234-1e0c-418a-a0d4-ef8b7501414d}`, IOCTL ADD (0x800) với W×H@Hz (mặc định 2800×1752@120 = Tab S10+), ping 0x888 mỗi ≤1 s (watchdog driver 3 s tự gỡ khi app chết), REMOVE khi dừng. GUID màn hình cố định → Windows nhớ scale.
- Engine: `--display virtual` (UI: "Màn hình ảo chính") tạo màn ảo + đặt làm màn hình chính (ChangeDisplaySettingsEx CDS_SET_PRIMARY). `extend` tạo màn ảo không đặt chính. `mirror` mà **không có output nào** → tự tạo màn ảo.
- GUI tự cài driver trước lần chia sẻ đầu nếu cần.

## Hệ quả / rủi ro
- Phụ thuộc bố cục installer Apollo; bước CI `continue-on-error`, thiếu thì build không kèm driver (UI báo).
- Chưa kiểm chứng trên máy thật (Windows 11 24H2, Radeon 890M). Có thể gặp Code 31/52 nếu chữ ký không được chấp nhận.
