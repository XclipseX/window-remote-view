# PROMPT NGHIÊN CỨU SÂU (≈2000 chữ) – ỨNG DỤNG ĐIỀU KHIỂN TỪ XA ANDROID → WINDOWS

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

## 5. Môi trường phần cứng và kịch bản sử dụng cần bao phủ
Nghiên cứu phải tính đến sự đa dạng thiết bị thực tế, không chỉ máy cấu hình cao:
- **Windows:** laptop văn phòng chỉ có GPU tích hợp Intel UHD/Iris Xe hoặc AMD Radeon Vega; máy chơi game có NVIDIA GeForce GTX 10xx đến RTX 50xx, AMD RX 6000–9000; máy ARM64 dùng Snapdragon X; máy cũ không có bộ mã hóa phần cứng, buộc phải dùng phần mềm. Với mỗi nhóm, hãy cho biết codec nào nên chọn mặc định và mức FPS, độ phân giải khả thi.
- **Android:** điện thoại giá rẻ dùng chip MediaTek Helio hoặc Unisoc, tầm trung Snapdragon 6/7 series, cao cấp Snapdragon 8 Gen, Dimensity 9000+, Google Tensor, Samsung Exynos; máy tính bảng màn hình lớn; Android TV box. Chỉ rõ thiết bị nào giải mã được AV1, HEVC bằng phần cứng và những lỗi đã biết của từng hãng.
- **Mạng:** Wi-Fi 5/6/6E cùng phòng, Wi-Fi qua tường, 4G/5G, mạng có CGNAT của nhà mạng Việt Nam, mạng công ty chặn UDP, kết nối chập chờn có tỷ lệ mất gói 1–5%.

Các kịch bản sử dụng điển hình cần đánh giá: làm việc văn phòng (gõ văn bản, Excel, cần chữ sắc nét, FPS thấp vẫn ổn); xem video và trình chiếu (cần chuyển động mượt); chơi game nhẹ hoặc điều khiển phần mềm đồ họa (cần độ trễ cực thấp, FPS cao); hỗ trợ kỹ thuật từ xa cho người thân (cần kết nối đơn giản nhất có thể, vượt được NAT); treo máy dài giờ để giám sát (cần tiết kiệm điện tối đa). Với mỗi kịch bản, hãy đề xuất một “cấu hình gợi ý” (preset) gồm codec, độ phân giải, FPS, mức thanh trượt chất lượng và chế độ nhập liệu, để người dùng chỉ cần bấm một nút.

## 5b. Thiết bị mục tiêu ưu tiên số 1 (phải tối ưu riêng)
Ngoài phần bao phủ chung, hãy nghiên cứu RIÊNG và CHI TIẾT cho hai thiết bị mà tôi dùng thực tế:
- **Samsung Galaxy S26+ dùng chip Exynos 2600** (GPU Xclipse, One UI trên Android 16).
- **Samsung Galaxy Tab S10+ dùng chip MediaTek Dimensity 9300+** (GPU Immortalis-G720, màn hình 12.4 inch 120 Hz, có S Pen).

Với mỗi thiết bị, cần trả lời:
1. Bộ giải mã phần cứng hỗ trợ những codec nào (H.264, HEVC Main/Main10, AV1, VP9), độ phân giải và FPS tối đa, có hỗ trợ YUV 4:4:4 không. Tên codec cụ thể trong MediaCodecList (ví dụ c2.exynos.*, c2.mtk.*) và cách kiểm tra.
2. Các khóa low-latency riêng của hãng (Samsung/Exynos vendor keys, MediaTek vendor keys) và lỗi đã biết khiến giải mã bị trễ thêm khung hình; cách khắc phục đã được cộng đồng Moonlight, scrcpy, Parsec, Steam Link ghi nhận.
3. Độ trễ giải mã thực tế ước tính (ms/khung) ở 1080p60, 1440p120, 2800x1752 (độ phân giải gốc Tab S10+).
4. Tối ưu pin và nhiệt: Game Optimizing Service, chế độ tiết kiệm của One UI, adaptive refresh rate, cách giữ 120 Hz ổn định mà không quá nóng khi dùng lâu.
5. Hỗ trợ S Pen (áp lực, nghiêng, nút bấm) truyền sang Windows Pen Injection, chế độ DeX, bàn phím Book Cover Keyboard, chuột Bluetooth với pointer capture.
6. Cấu hình mặc định đề xuất (codec, độ phân giải, FPS, bitrate) cho từng máy trong LAN và qua Internet.
Lưu ý: nếu thông tin về Exynos 2600 còn hạn chế, hãy nêu rõ và suy luận từ Exynos 2400/2500 kèm mức độ chắc chắn.

## 5c. Hai chế độ kết nối bắt buộc: LAN và Server
Ứng dụng phải hỗ trợ đầy đủ CẢ HAI chế độ và tự động chọn đường tốt nhất:
- **Chế độ LAN (không cần Internet):** tự phát hiện máy Windows trong cùng mạng bằng mDNS/DNS-SD hoặc UDP broadcast; kết nối trực tiếp bằng IP; ghép đôi bằng mã PIN hoặc QR một lần rồi ghi nhớ; hoạt động được khi mất Internet, qua Wi-Fi Direct, hotspot điện thoại hoặc cáp USB tethering. Tối ưu riêng cho LAN: bitrate cao (50–150 Mbps), FEC thấp, bộ đệm gần bằng 0.
- **Chế độ Server (qua Internet):** máy chủ tự host miễn phí gồm signaling/ID server, STUN và relay (TURN hoặc kiểu hbbs/hbbr của RustDesk), triển khai bằng Docker trên VPS free tier hoặc máy nhà; ưu tiên P2P hole punching, chỉ relay khi thất bại; hỗ trợ IPv6; vượt CGNAT nhà mạng Việt Nam.
- **Chuyển đổi liền mạch:** khi điện thoại rời Wi-Fi sang 4G/5G thì phiên tự chuyển từ LAN sang Server mà không ngắt (QUIC connection migration hoặc cơ chế tương tự), và ngược lại.
- So sánh độ trễ, băng thông, chi phí của từng chế độ; đề xuất thứ tự ưu tiên khi kết nối (LAN trực tiếp → P2P → relay).

## 6. Danh sách câu hỏi cụ thể cần trả lời
1. Với cùng một mức chất lượng cảm nhận ở 1080p60 nội dung văn phòng, H.264, HEVC và AV1 cần bitrate lần lượt khoảng bao nhiêu? Chênh lệch điện năng giải mã trên Android ra sao?
2. Có nên dùng mã hóa YUV 4:4:4 hoặc chế độ “screen content coding” để chữ sắc nét hơn không? Phần cứng nào hỗ trợ và đổi lại tốn bao nhiêu băng thông?
3. Độ trễ đóng góp của từng khâu (chụp, mã hóa, mạng, bộ đệm, giải mã, hiển thị) thường là bao nhiêu mili giây, và khâu nào dễ cắt giảm nhất?
4. Làm sao xử lý khi người dùng xoay màn hình điện thoại, thay đổi độ phân giải Windows hoặc cắm thêm màn hình giữa phiên kết nối mà không bị gián đoạn?
5. Cách đơn giản nhất để kết nối qua Internet mà không cần người dùng mở cổng router hay cấu hình gì, trong khi vẫn giữ chi phí máy chủ gần bằng không?
6. Ứng dụng có thể truyền âm thanh hệ thống từ Windows sang Android với độ trễ thấp bằng codec Opus không? Nếu có, đồng bộ âm thanh và hình ảnh thế nào?
7. Có cần ứng dụng Android sử dụng AccessibilityService hoặc quyền đặc biệt nào không, và điều đó ảnh hưởng gì tới việc phát hành trên Google Play?
8. Những bẫy pháp lý nào cần tránh khi phát hành một ứng dụng điều khiển từ xa miễn phí, chẳng hạn bị lạm dụng để lừa đảo, và nên thiết kế tính năng nào để hạn chế điều đó?

## 7. Tiêu chí đánh giá phương án
Khi so sánh các lựa chọn, hãy chấm điểm từ 1 đến 5 theo các tiêu chí: độ trễ, mức tiêu thụ điện, chất lượng hình ảnh, độ ổn định, độ phức tạp khi triển khai, khả năng bảo trì lâu dài, mức độ tương thích thiết bị và rủi ro giấy phép. Đưa ra tổng điểm có trọng số, trong đó độ trễ và điện năng có trọng số cao nhất, sau đó là chất lượng và độ dễ dùng.

## 8. Yêu cầu đầu ra
Trình bày bằng tiếng Việt, giữ nguyên thuật ngữ kỹ thuật tiếng Anh. Mọi khẳng định quan trọng phải có nguồn. Ưu tiên giải pháp miễn phí, mã nguồn mở, tiêu tốn ít tài nguyên; không đề xuất SDK hoặc codec trả phí. Bao gồm bảng so sánh, số liệu ước lượng và mã mẫu ngắn cho các phần then chốt: chụp màn hình sang NVENC, cấu hình MediaCodec độ trễ thấp, SendInput và Touch Injection, công thức thanh trượt chất lượng sang bitrate. Kết thúc bằng một bản tóm tắt điều hành dài khoảng một trang.
