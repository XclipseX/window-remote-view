---
type: decision
status: accepted
date: 2026-11-27
tags: [windows, service, lock-screen, security]
---
# ADR-0004: Dịch vụ SYSTEM để xem/điều khiển màn hình khóa

## Bối cảnh
Khi Windows khóa (lock screen, đăng nhập, UAC) desktop nhập liệu là `Winlogon` (secure desktop).
Worker chạy dưới quyền user không `OpenInputDesktop`/DXGI-duplicate/`SendInput` được → ảnh đứng hoặc đen,
điều khiển bị chặn (`LOCKED_MESSAGE`). TeamViewer, Sunshine/Apollo xử lý được vì chạy một tiến trình
LocalSystem bên trong phiên console của user.

## Quyết định
* `RemoteView.exe --service` = dịch vụ Windows `RemoteViewSvc` (auto start, LocalSystem), lắng nghe
  named pipe `\\.\pipe\RemoteViewSvc`. Mỗi client yêu cầu `{"op":"spawn","args":[...]}`; dịch vụ chạy
  **chính exe của nó** với `--host-worker` bằng token SYSTEM nhân bản, đổi `TokenSessionId` sang phiên
  console đang hoạt động, `CreateProcessAsUserW` (desktop `winsta0\default`) rồi chuyển tiếp từng dòng JSON.
* GUI (`apps/win-ui/src/host.rs`) dùng pipe nếu kết nối được, nếu không thì quay về tiến trình con như cũ.
  `HostCmd`/`HostEvent` không đổi.
* Worker chạy SYSTEM (`session_state::running_as_system`) gắn từng luồng (capture, cursor, input, monitor)
  vào desktop nhập liệu bằng `follow_input_desktop()` (`OpenInputDesktop` + `SetThreadDesktop`) và mở lại
  DXGI duplication khi desktop đổi. Cờ `locked` không còn chặn capture/input ở chế độ này. Không SYSTEM:
  hành vi y như trước.
* Cài một lần cùng SudoVDA qua `--install-all <dir|->` (một hộp UAC); gỡ bằng `--uninstall-service`.
* Không thêm crate: chỉ dùng crate `windows` (Services, RemoteDesktop, Pipes, Security, Environment).
  Pipe được bơm bằng một luồng polling (`PeekNamedPipe`) vì handle pipe đồng bộ tuần tự hóa read/write.

## Bảo mật
* ACL pipe `D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)`: SYSTEM + Administrators toàn quyền, user tương tác
  chỉ đọc/ghi; từ chối client từ xa; một instance duy nhất, `FILE_FLAG_FIRST_PIPE_INSTANCE` chống chiếm tên.
* Danh sách cho phép tham số (`rv_host_win::service_proto::validate_worker_args`): chỉ cờ của engine
  (`--usb --adb --lan --json-status --display --mode --fps --width --height --quality --codec --encoder
  --bitrate-mbps --otp --monitor`) với giá trị đúng dạng; cấm `--host-path`, `--bind`, đường dẫn,
  `--list`, `--scan-all`... và engine tự parse lại lần nữa. Dịch vụ luôn chạy exe của chính nó, không chạy
  đường dẫn do client đưa.
* **Rủi ro còn lại:** bất kỳ user tương tác nào trên máy (kể cả không phải admin) đều khởi chạy được một
  worker SYSTEM với các cờ cho phép, tức là chia sẻ màn hình (kể cả màn hình khóa) và bơm input bằng
  SYSTEM. Việc ghép đôi vẫn cần OTP/token như trước, nhưng user cục bộ khác có thể tự chạy worker với
  `--otp` do mình chọn. Chấp nhận vì máy là laptop cá nhân một người dùng; nếu cần chặt hơn: giới hạn
  pipe cho SID của user đang đăng nhập console, hoặc thêm token bí mật đọc từ file chỉ admin ghi.
  Dịch vụ trỏ vào đường dẫn exe lúc cài: nếu exe nằm ở thư mục user ghi được, user đó thay exe là chiếm được
  SYSTEM — nên đặt app ở nơi chỉ admin ghi (Program Files) khi triển khai.

## Chưa kiểm chứng trên Windows thật
Toàn bộ đường SYSTEM/Winlogon (DXGI trên secure desktop, `SendInput` vào Winlogon, cảm ứng/bút tổng hợp
`InjectSyntheticPointerInput`) mới được biên dịch chéo, chưa chạy.
