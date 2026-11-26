---
type: decision
date: 2026-11-26
---
# ADR-0001: Ký APK bằng khóa cố định trong repo

- Bối cảnh: CI tạo khóa debug ngẫu nhiên mỗi lần → APK mới không cài đè được bản cũ.
- Quyết định: `apps/android/app/debug.keystore` (mật khẩu `android`) commit vào repo; versionCode = số lần chạy CI. Khóa phát hành thật đọc từ secrets `RV_KEYSTORE_*` nếu có, không lưu trong repo.
- Hệ quả: khóa debug là công khai, không dùng cho Play Store. Bản đã cài với chữ ký cũ phải gỡ một lần.
