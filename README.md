# window-remote-view
Window remote view

## Windows app (một file) và driver USB

Bản phát hành chỉ có `RemoteView.exe` (+ APK). Cắm cáp USB, thiết bị hiện thành thẻ có nút "Kết nối";
không cần Zadig, không cần nhập PIN cho USB (máy tính gửi mã ghép nối ngẫu nhiên 8 số qua chuỗi
nhận dạng AOA, `serial`). PIN chỉ dùng cho LAN. Lần đầu RemoteView tự cài driver WinUSB bằng
`wdi-simple.exe` (libwdi) với quyền quản trị.

Giấy phép bên thứ ba: xem [NOTICE](NOTICE).
