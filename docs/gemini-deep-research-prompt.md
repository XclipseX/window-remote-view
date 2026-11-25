# PROMPT NGHIÊN CỨU SÂU (≈1500 chữ) – ỨNG DỤNG ĐIỀU KHIỂN TỪ XA ANDROID → WINDOWS

## 1. Vai trò
Bạn là một nhóm kỹ sư trưởng gồm chuyên gia video streaming thời gian thực, codec phần cứng, lập trình hệ thống Windows, Android native, mạng độ trễ thấp và tối ưu năng lượng. Nhiệm vụ của bạn là thực hiện một nghiên cứu kỹ thuật chuyên sâu, có trích dẫn nguồn đáng tin cậy (tài liệu chính thức của Microsoft, Google, NVIDIA, AMD, Intel; mã nguồn GitHub; bài báo khoa học; benchmark công khai), ưu tiên thông tin mới nhất giai đoạn 2025–2026. Khi một thông tin chưa được kiểm chứng, hãy ghi rõ mức độ chắc chắn.

## 2. Bối cảnh và mục tiêu
Tôi muốn xây dựng một ứng dụng điều khiển máy tính từ xa tương tự TeamViewer nhưng hoàn toàn miễn phí, mã nguồn mở, không giới hạn phiên, không quảng cáo và không phụ thuộc vào máy chủ trả phí. Ứng dụng chỉ tập trung vào hai chức năng cốt lõi:
1. Cài đặt được trên Windows 10/11 (x64 và ARM64) và Android 8.0 trở lên.
2. Cho phép điện thoại hoặc máy tính bảng Android điều khiển máy tính Windows một cách dễ dàng, mượt mà.

Các tiêu chí tối ưu theo thứ tự ưu tiên: độ trễ thấp (mục tiêu dưới 30 ms trong mạng LAN và dưới 80 ms qua Internet), tiết kiệm điện năng (pin Android và tải CPU/GPU trên Windows), chất lượng hình ảnh sắc nét, tốc độ khung hình cao (30, 60, 90, 120, 144 FPS). Ứng dụng phải dễ sử dụng với người không rành kỹ thuật: kết nối bằng ID và mật khẩu, quét mã QR hoặc tự phát hiện máy trong mạng LAN.

## 3. Yêu cầu kỹ thuật bắt buộc
- Chỉ sử dụng codec mở hoặc phổ biến, không dùng thuật toán độc quyền ngốn tài nguyên như TeamViewer. Cần đánh giá H.264, H.265/HEVC, VP9 và AV1, bao gồm cả vấn đề bằng sáng chế và giấy phép khi phân phối miễn phí.
- Hỗ trợ cả mã hóa/giải mã bằng phần cứng (NVENC, AMF, Intel QuickSync/oneVPL, Media Foundation, D3D12 Video, Android MediaCodec) và phần mềm native làm phương án dự phòng (x264, OpenH264, libvpx, SVT-AV1, dav1d, FFmpeg).
- Người dùng tùy chỉnh được độ phân giải, FPS và chất lượng hình ảnh bằng thanh trượt từ 1% đến 100%. Hãy đề xuất công thức ánh xạ giá trị thanh trượt sang bitrate, QP hoặc CRF theo độ phân giải và FPS, kèm chế độ tự động thích ứng băng thông.
- Hỗ trợ đầy đủ các phương thức nhập liệu: bàn phím ảo, bàn phím vật lý hoặc Bluetooth, phím đặc biệt (Ctrl+Alt+Del, phím Windows, F1–F12, tổ hợp phím), gõ tiếng Việt và Unicode, cảm ứng đa điểm, cử chỉ, chế độ trackpad rê chuột tương đối, chế độ chạm trực tiếp tuyệt đối, chuột Bluetooth/OTG, bút cảm ứng có lực nhấn, đồng bộ clipboard.

## 4. Phân chia agent nghiên cứu
Hãy tổ chức nghiên cứu thành các agent chuyên trách. Mỗi agent trả về: phương án đề xuất, bảng so sánh ít nhất ba lựa chọn, rủi ro và cách giảm thiểu, đoạn mã mẫu hoặc API cụ thể, danh sách nguồn có đường dẫn.

**Agent 1 – Kiến trúc và dự án tham khảo.** Phân tích RustDesk, Sunshine kết hợp Moonlight, scrcpy, Parsec (phần công khai), Chrome Remote Desktop, Apache Guacamole và Selkies. Đánh giá điểm mạnh, điểm yếu về độ trễ, CPU, pin; xem xét giấy phép (GPL, AGPL, MIT, Apache) để quyết định nên tự viết mới hay fork. So sánh ngôn ngữ Rust, C++, Kotlin với NDK và các framework giao diện. Vẽ sơ đồ luồng: chụp màn hình → mã hóa → truyền tải → giải mã → hiển thị, cùng luồng nhập liệu theo chiều ngược lại.

**Agent 2 – Chụp màn hình Windows.** So sánh DXGI Desktop Duplication, Windows.Graphics.Capture và GDI. Thiết kế đường đi zero-copy từ texture Direct3D 11 vào bộ mã hóa phần cứng, không qua RAM CPU. Xử lý đa màn hình, HDR, DPI scaling, màn hình khóa, UAC, Secure Desktop (chạy dưới dạng Windows Service), máy không gắn màn hình (virtual display driver IddCx), gửi hình con trỏ riêng và chỉ mã hóa khi có vùng thay đổi để tiết kiệm điện.

**Agent 3 – Mã hóa video trên Windows.** So sánh NVENC, AMF, QuickSync, Media Foundation và D3D12 Video Encode với H.264, HEVC, AV1. Đưa ra cấu hình độ trễ thấp chi tiết: không dùng B-frame, intra-refresh thay cho keyframe định kỳ, bộ đệm VBV nhỏ, chia slice, tắt lookahead, chọn CBR hoặc VBR. Đo chi phí CPU của phương án phần mềm ở 1080p60. Phân tích giấy phép từng codec và đề xuất công thức thanh trượt chất lượng.

**Agent 4 – Giải mã và hiển thị trên Android.** Nghiên cứu MediaCodec qua NDK ở chế độ độ trễ thấp (KEY_LOW_LATENCY, KEY_PRIORITY, khóa riêng của Qualcomm, MediaTek, Exynos, Tensor), xuất trực tiếp vào SurfaceView. Tự động chọn codec theo khả năng thiết bị, dự phòng bằng dav1d hoặc FFmpeg. Đồng bộ khung hình với Choreographer, tận dụng màn hình 90/120 Hz, giảm FPS khi hình tĩnh, khóa Wi-Fi độ trễ thấp và tạm dừng khi ứng dụng chạy nền.

**Agent 5 – Mạng và truyền tải.** So sánh UDP thuần kèm FEC, WebRTC, QUIC, KCP, SRT và TCP. Thiết kế xuyên NAT bằng STUN, UDP hole punching, máy chủ trung chuyển tự dựng (coturn hoặc tương tự RustDesk hbbs/hbbr), khám phá LAN bằng mDNS. Đề xuất thuật toán kiểm soát nghẽn, thích ứng bitrate, sửa lỗi Reed-Solomon, ưu tiên kênh nhập liệu. Tính toán chi phí vận hành gần bằng không.

**Agent 6 – Nhập liệu và trải nghiệm điều khiển.** Phía Windows: SendInput, scan code và virtual key, Touch Injection, Pen Injection, gửi Ctrl+Alt+Del qua quyền SYSTEM, nhập Unicode. Phía Android: thiết kế cử chỉ (chạm để nhấp, giữ để nhấp phải, hai ngón để cuộn, chụm để phóng to), thanh phím đặc biệt nổi, pointer capture cho chuột ngoài, bộ gõ tiếng Việt, hiển thị con trỏ cục bộ để giảm cảm giác trễ.

**Agent 7 – Bảo mật.** Mã hóa đầu cuối bằng TLS 1.3, DTLS hoặc Noise Protocol; xác thực bằng mật khẩu một lần và khóa công khai; chống dò mật khẩu; yêu cầu xác nhận trên Windows; không lưu dữ liệu người dùng trên máy chủ.

**Agent 8 – Đo lường hiệu năng và năng lượng.** Đề xuất phương pháp đo độ trễ từ màn hình đến màn hình (camera tốc độ cao, cảm biến quang), công cụ PresentMon, GPUView, ETW, Perfetto, Battery Historian. Lập bảng mục tiêu bitrate, FPS, độ trễ, công suất cho 720p30, 1080p60, 1440p120 và 4K60; so sánh với TeamViewer, AnyDesk, RustDesk, Parsec, Moonlight.

**Agent 9 – Đóng gói, phân phối và giao diện.** Trình cài đặt Windows (MSIX, WiX, Inno Setup), chạy nền như dịch vụ, ký mã miễn phí cho dự án mã nguồn mở, tránh bị SmartScreen chặn. Phát hành APK qua GitHub Releases, F-Droid, Google Play. Giao diện tối giản: màn hình chính chỉ có “ID của tôi” và “Kết nối tới”; trang cài đặt gồm độ phân giải, FPS, thanh trượt chất lượng, chọn codec, chọn phần cứng hoặc phần mềm. Thiết lập CI/CD bằng GitHub Actions.

**Agent 10 – Kiến trúc sư tổng hợp.** Tổng hợp kết quả thành: kiến trúc cuối cùng kèm lý do; tech stack với phiên bản và giấy phép; lộ trình từ MVP đến bản 1.0 chia thành các sprint hai tuần với tiêu chí đo được; cấu trúc thư mục monorepo; định nghĩa giao thức gói tin video, nhập liệu, điều khiển; danh sách rủi ro lớn nhất và các quyết định còn bỏ ngỏ.

## 5. Yêu cầu đầu ra
Trình bày bằng tiếng Việt, giữ nguyên thuật ngữ kỹ thuật tiếng Anh. Mọi khẳng định quan trọng phải có nguồn. Ưu tiên giải pháp miễn phí, mã nguồn mở, tiêu tốn ít tài nguyên; không đề xuất SDK hoặc codec trả phí. Bao gồm bảng so sánh, số liệu ước lượng và mã mẫu ngắn cho các phần then chốt: chụp màn hình sang NVENC, cấu hình MediaCodec độ trễ thấp, SendInput và Touch Injection, công thức thanh trượt chất lượng sang bitrate. Kết thúc bằng một bản tóm tắt điều hành dài khoảng một trang.
