# **Thiết kế kỹ thuật chuyên sâu ứng dụng điều khiển máy tính từ xa Android đến Windows độ trễ thấp, tiết kiệm năng lượng và mã nguồn mở**

Để xây dựng một ứng dụng điều khiển máy tính từ xa mã nguồn mở, hoàn toàn miễn phí từ Android 8.0+ sang Windows 10/11 (x64 và ARM64) đạt mục tiêu độ trễ dưới 30 ms trong mạng LAN, dưới 80 ms qua Internet và tối ưu pin di động, kiến trúc đề xuất là phát triển mới lõi streaming đa nền tảng bằng **Rust** (giấy phép kép **MIT/Apache-2.0**) kết hợp giao diện thuần bản địa mỏng (**Tauri v2/WinUI 3** trên Windows và **Kotlin/Jetpack Compose \+ NDK** trên Android), thay vì fork các dự án copyleft mạnh như Sunshine (GPL-3.0) hay RustDesk (AGPL-3.0). Đường ống xử lý giữ khung hình hoàn toàn trong GPU VRAM theo cơ chế zero-copy từ **DXGI Desktop Duplication** sang bộ mã hóa phần cứng bản địa (**NVENC, AMF, Intel oneVPL**, và **Media Foundation** cho ARM64), truyền tải qua **UDP tùy chỉnh có sửa lỗi Reed-Solomon thích ứng** kèm kênh dự phòng **TCP/443**, mã hóa đầu cuối bằng **Noise Protocol (Noise\_XX/Noise\_IK)**, và giải mã phần cứng trực tiếp ra **SurfaceView** qua **MediaCodec** chế độ độ trễ thấp trên Android.  
Về chiến lược codec và chi phí vận hành, hệ thống triển khai bậc thang ba codec: **H.264** đảm bảo tương thích tuyệt đối (phần cứng sẵn có \+ dự phòng phần mềm qua nhị phân **OpenH264** do Cisco tài trợ bản quyền), **HEVC (H.265)** tối ưu hiệu suất năng lượng và băng thông (chỉ kích hoạt qua phần cứng OS/GPU nhằm loại bỏ rủi ro bằng sáng chế phần mềm), và **AV1** miễn phí bản quyền cho phần cứng 2022–2026. Ở kịch bản văn phòng 1080p60 kết hợp bỏ qua khung hình tĩnh (GetFrameDirtyRects), băng thông thực tế trung bình chỉ 1,2–5,0 Mbps tùy codec. Bằng cách ưu tiên kết nối trực tiếp qua **mDNS** (LAN), **IPv6 Direct** (tận dụng tỷ lệ truy cập IPv6 tại Việt Nam vượt 45% đầu năm 2026 và mục tiêu 72% trong năm 2026 để vượt CGNAT IPv4) và **STUN/UDP hole punching** trên cùng một socket, khoảng 85% phiên chạy P2P trực tiếp không tốn băng thông máy chủ; theo mô hình ước lượng kỹ thuật cho \~15% phiên còn lại đi qua máy chủ Relay tự dựng, chi phí vận hành cho 1.000 người dùng hoạt động có thể giữ ở mức 0–4,50 USD/tháng.  
Về mức độ chắc chắn dữ liệu, các mốc độ trễ LAN dưới 15 ms (như bài đo độc lập tháng 5/2026 của Tenvo ghi nhận Moonlight đạt trung vị 8–12 ms trên Ethernet 1 Gbps, hay công bố từ nhà phát triển Parsec khoảng 7 ms trên LAN có dây) đại diện cho điều kiện mạng và phần cứng thuận lợi. Trên điện thoại Android giá rẻ hoặc mạng di động CGNAT, độ trễ và độ ổn định chịu chi phối mạnh bởi trình điều khiển giải mã của từng hãng SoC — điển hình là lỗi hồi quy màn hình đen với HEVC trên MediaTek Dimensity khi nâng cấp Android 15 hay độ trễ giải mã 20–28 ms được cộng đồng phản ánh trên một số chip MediaTek. Do đó, toàn bộ thiết kế dưới đây tích hợp sẵn cơ chế kiểm tra năng lực thực tế, danh sách loại trừ lỗi phần cứng (device quirks) và thang hạ cấp tự động.

## **Hệ thống tiêu chí và trọng số đánh giá**

Mọi bảng so sánh phương án trong 10 agent chuyên trách đều áp dụng thang điểm 1 đến 5 (5 là tốt nhất) và tổng điểm có trọng số thống nhất theo đúng ưu tiên của dự án: độ trễ và điện năng chiếm 45% tổng trọng số.

| Tiêu chí đánh giá | Ký hiệu | Trọng số | Ý nghĩa thang điểm 1–5 |
| :---- | :---- | :---- | :---- |
| Độ trễ (Latency) | **Trễ** | **25%** | 5 \= siêu thấp (\<15 ms xử lý); 1 \= trễ lớn (\>100 ms) |
| Mức tiêu thụ điện (Power) | **Điện** | **20%** | 5 \= phần cứng chuyên dụng, zero-copy; 1 \= tải CPU/pin cao |
| Chất lượng hình ảnh (Quality) | **Ảnh** | **15%** | 5 \= sắc nét, hỗ trợ 10-bit/HDR/chữ rõ; 1 \= mờ/nhòe |
| Độ ổn định & Dễ dùng (Stability/UX) | **Ổn định** | **12%** | 5 \= bền bỉ, tự phục hồi lỗi mạng/driver; 1 \= dễ treo |
| Mức độ tương thích thiết bị (Compatibility) | **Tương thích** | **10%** | 5 \= phủ rộng Windows 10/11 & Android 8+; 1 \= kén máy |
| Khả năng bảo trì lâu dài (Maintainability) | **Bảo trì** | **8%** | 5 \= kiến trúc sạch, hệ sinh thái khỏe; 1 \= nợ kỹ thuật lớn |
| Độ đơn giản triển khai (Simplicity) | **Đơn giản** | **5%** | 5 \= ít công sức triển khai; 1 \= rất phức tạp |
| Rủi ro giấy phép & pháp lý (License) | **Pháp lý** | **5%** | 5 \= hoàn toàn mở, không rủi ro; 1 \= copyleft gắt/bản quyền |

## **Agent 1 – Kiến trúc tổng thể và đánh giá dự án tham khảo**

### **Phương án đề xuất**

Phương án tối ưu là **viết mới lõi streaming dùng chung bằng Rust** (giấy phép kép **MIT/Apache-2.0**) và ghép với **giao diện bản địa siêu nhẹ**: **Tauri v2 (WebView2) hoặc WinUI 3** trên Windows và **Kotlin/Jetpack Compose \+ Android NDK** trên Android. Thay vì fork nguyên khối một kho mã nguồn dính giấy phép copyleft mạnh, dự án học hỏi trực tiếp bốn hình mẫu kỹ thuật đã kiểm chứng: kiến trúc đa tiến trình tách đặc quyền của **Chrome Remote Desktop**, đường ống zero-copy độ trễ cực thấp của **Sunshine/Moonlight**, cơ chế điều phối ID/Relay tự dựng của **RustDesk** và sự gọn nhẹ của **scrcpy**.

### **Đánh giá 7 dự án tham khảo cốt lõi**

Các dự án điều khiển từ xa hiện hữu giai đoạn 2025–2026 bộc lộ sự đánh đổi rõ rệt giữa độ trễ, tính dễ dùng qua WAN và ràng buộc giấy phép. **RustDesk** (Rust, **AGPL-3.0**) rất thành công về trải nghiệm ID/mật khẩu và máy chủ tự dựng hbbs/hbbr, nhưng phản hồi cộng đồng cho thấy RustDesk kém mượt hơn Sunshine+Moonlight ở các tác vụ chuyển động nhanh hoặc điều khiển camera game, đồng thời giấy phép AGPL-3.0 bắt buộc công khai mã nguồn bất kỳ sửa đổi máy chủ nào. **Sunshine kết hợp Moonlight** (C++, **GPL-3.0**) dẫn đầu thế giới mã nguồn mở về độ trễ (trung vị 8–12 ms trên LAN trong bài đo tháng 5/2026) và hỗ trợ miễn phí tới 4K HDR 120 FPS (bản fork **Apollo** còn tích hợp màn hình ảo), nhưng không có sẵn cơ chế xuyên NAT qua Internet cho người phổ thông — buộc người dùng phải tự mở cổng router hoặc cấu hình Tailscale/WireGuard.  
Ở nhóm dự án khác, **scrcpy** (C, **Apache-2.0**) đạt hiệu năng cao và nhẹ nhưng đi theo chiều ngược lại (Android sang PC qua ADB). **Parsec** (mã nguồn đóng, lõi C thuần) theo tài liệu kỹ thuật của hãng chỉ thêm \~7 ms trên LAN Ethernet nhờ giao thức UDP riêng **BUD (Better User Datagrams)** có tỷ lệ xuyên NAT 97% và mã hóa DTLS 1.2 AES-128/256, nhưng bắt buộc tài khoản tập trung và khóa các tính năng 4:4:4, đa màn hình, bút vẽ Wacom ở gói trả phí Warp (8,33–9,99 USD/tháng). **Chrome Remote Desktop** (mã nguồn host trong Chromium nhưng phụ thuộc dịch vụ đám mây Google) cung cấp kiến trúc bảo mật Windows mẫu mực: daemon\_process quyền SYSTEM làm môi giới Mojo IPC, tiến trình mạng LOCAL\_SERVICE đặc quyền thấp để nhận gói Internet, và desktop\_process quyền SYSTEM chụp Secure Desktop/UAC. **Apache Guacamole** (mã nguồn mở cổng HTML5) dựa trên RDP/VNC qua trình duyệt nên độ trễ cao (50–150+ ms), không đáp ứng mục tiêu \<30 ms và 120–144 FPS. **Selkies** (Rust/Python, **MPL-2.0**) tối ưu cho desktop container Linux/Kubernetes qua WebRTC chứ không chuyên biệt cho máy Windows cá nhân và tối ưu pin Android native.

### **Bảng so sánh chiến lược xây dựng và ngôn ngữ/UI**

| Phương án chiến lược (Agent 1a) | Trễ (25%) | Điện (20%) | Ảnh (15%) | Ổn định (12%) | Tương thích (10%) | Bảo trì (8%) | Đơn giản (5%) | Pháp lý (5%) | Tổng điểm |
| :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- |
| **Viết mới Rust Core \+ Native UI (MIT/Apache-2.0)** | 5 | 5 | 5 | 4 | 5 | 5 | 2 | 5 | **4,73** |
| Fork Sunshine \+ Moonlight (GPL-3.0) | 5 | 4 | 5 | 5 | 4 | 3 | 3 | 2 | **4,29** |
| Fork RustDesk (AGPL-3.0) | 3 | 3 | 4 | 4 | 5 | 3 | 4 | 2 | **3,47** |
| Fork Selkies (MPL-2.0, WebRTC) | 3 | 3 | 4 | 3 | 4 | 4 | 3 | 4 | **3,38** |
| Fork Apache Guacamole (HTML5 Gateway) | 1 | 2 | 2 | 4 | 5 | 4 | 4 | 5 | **2,70** |

| Ngôn ngữ lõi & Framework UI (Agent 1b) | Trễ (25%) | Điện (20%) | Ảnh (15%) | Ổn định (12%) | Tương thích (10%) | Bảo trì (8%) | Đơn giản (5%) | Pháp lý (5%) | Tổng điểm |
| :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- |
| **Rust Core \+ WinUI 3/Tauri (Win) \+ Kotlin/Compose & NDK (And)** | 5 | 5 | 5 | 5 | 5 | 5 | 3 | 5 | **4,90** |
| C++20 Core \+ Qt6/Win32 (Win) \+ Kotlin/NDK (And) | 5 | 5 | 5 | 4 | 5 | 3 | 2 | 4 | **4,52** |
| Rust Core \+ Slint / egui UI | 5 | 4 | 4 | 3 | 3 | 4 | 4 | 4 | **4,03** |
| Rust Core \+ Flutter UI (Đa nền tảng) | 4 | 3 | 4 | 4 | 5 | 4 | 4 | 5 | **3,95** |

*Nguồn điểm tổng hợp có trọng số:.*  
Rust kết hợp UI bản địa thắng thuyết phục nhờ ba yếu tố: không có Garbage Collector nên triệt tiêu khựng khung hình (frame pacing stutter); đảm bảo an toàn bộ nhớ ở tầng phân tích gói mạng và dịch vụ SYSTEM Windows; và giữ kích thước bộ cài siêu gọn (Tauri dùng WebView2 sẵn có trên Windows chỉ khoảng 3–5 MB so với \>150 MB của Electron, còn Android dùng trực tiếp SurfaceView bản địa thay vì lớp render trung gian của Flutter).

### **Sơ đồ luồng dữ liệu hai chiều đầu cuối**

Kiến trúc phân tách rõ hai luồng độc lập: đường ống media chiều đi chạy hoàn toàn trên GPU VRAM/hardware decoder và đường ống nhập liệu ưu tiên cao chạy theo chiều ngược lại về Windows Service quyền SYSTEM.

### **Giao diện lõi Rust chia sẻ đa nền tảng (API mẫu)**

// Biên trừu tượng lõi trong /crates/proto giúp tách biệt hoàn toàn nền tảng OS khỏi giao thức  
pub trait FrameCapturer: Send {  
    fn acquire\_gpu\_frame(\&mut self, timeout\_ms: u32) \-\> Result\<Option\<GpuFrameHandle\>, CaptureError\>;  
}

pub trait VideoEncoder: Send {  
    fn encode\_gpu\_handle(\&mut self, frame: \&GpuFrameHandle, force\_idr: bool) \-\> Result\<EncodedPacket, EncodeError\>;  
    fn update\_rate\_control(\&mut self, bitrate\_kbps: u32, target\_qp: u8) \-\> Result\<(), EncodeError\>;  
}

### **Rủi ro và giảm thiểu**

| Rủi ro kiến trúc | Tác động | Biện pháp giảm thiểu |
| :---- | :---- | :---- |
| Lỗi rò rỉ tài nguyên COM/JNI qua ranh giới FFI | Crash đột ngột hoặc tăng dần VRAM/RAM | Dùng windows-rs tự động Release() COM trên Windows; trên Android giữ ranh giới C-ABI hẹp chỉ truyền byte buffer đã đóng khung và handle ANativeWindow. |
| Tiến trình chụp màn hình gặp lỗi driver làm sập toàn bộ dịch vụ | Mất phiên điều khiển không thể nối lại từ xa | Áp dụng mô hình đa tiến trình (kiểu CRD): Service SYSTEM giám sát và tự khởi chạy lại worker chụp màn hình trong \<500 ms. |

**Danh sách nguồn Agent 1:**

> * So sánh RustDesk và scrcpy (AGPL-3.0 vs Apache-2.0): https\://www\.libhunt.com/compare-scrcpy-vs-rustdesk  
> * Giấy phép AGPL-3.0 RustDesk: https\://github.com/rustdesk/rustdesk/blob/master/LICENCE  
> * Kho mã nguồn Sunshine (GPL-3.0): https\://github.com/SunshineStream/Sunshine  
> * Hệ sinh thái Selkies (MPL-2.0): https\://github.com/orgs/selkies-project/repositories  
> * Kiến trúc đa tiến trình Chrome Remote Desktop: https\://chromium.googlesource.com/chromium/src/+/refs/tags/147.0.7727.0/remoting/GEMINI.md  
> * Kiến trúc Parsec & giao thức BUD: https\://support.parsec.app/hc/en-us/articles/32361354307348-Overview  
> * So sánh WinUI 3 và Tauri: https\://softwarelogic.co/en-US/knowledge-base/winui-3-vs-qt-comprehensive-guide-for-modern-enterprise-apps/ và https\://teamdev.com/mobrowser/blog/top-7-desktop-app-frameworks-in-2026/

## **Agent 2 – Chụp màn hình Windows tối ưu Zero-Copy**

### **Phương án đề xuất**

Lựa chọn **DXGI Desktop Duplication API (IDXGIOutputDuplication)** làm phương thức mặc định nhờ lấy khung hình trực tiếp từ VRAM trong dưới 1 ms, hỗ trợ truy xuất vùng màn hình thay đổi (GetFrameDirtyRects) và con trỏ chuột phần cứng tách biệt. Sử dụng **Windows.Graphics.Capture (WGC)** làm phương án dự phòng tự động cho laptop hai GPU (Optimus/Hybrid Graphics) nơi màn hình gắn vào iGPU nhưng cần chụp xuyên GPU. Chỉ giữ **GDI BitBlt** làm phương án cứu cánh cuối cùng cho máy ảo hoặc trình điều khiển đồ họa lỗi.

### **Bảng so sánh API chụp màn hình Windows**

| API chụp màn hình (Agent 2\) | Trễ (25%) | Điện (20%) | Ảnh (15%) | Ổn định (12%) | Tương thích (10%) | Bảo trì (8%) | Đơn giản (5%) | Pháp lý (5%) | Tổng điểm |
| :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- |
| **DXGI Desktop Duplication (D3D11/12)** | 5 | 5 | 5 | 5 | 4 | 4 | 3 | 5 | **4,72** |
| Windows.Graphics.Capture (WGC) | 4 | 4 | 5 | 4 | 4 | 5 | 4 | 5 | **4,28** |
| GDI BitBlt (Dự phòng khẩn cấp) | 1 | 1 | 3 | 5 | 5 | 5 | 5 | 5 | **2,90** |

*Cơ sở đánh giá: DXGI cho zero-copy trực tiếp và dirty rectangles; WGC linh hoạt xuyên GPU và tắt được viền vàng trên Windows 11 nhưng không trả trực tiếp danh sách dirty rects.*

### **Đường ống Zero-Copy và xử lý các tình huống hệ thống phức tạp**

Để loại bỏ hoàn toàn sao chép qua bus PCIe vào RAM CPU, khung hình ID3D11Texture2D lấy từ AcquireNextFrame() được xử lý chuyển không gian màu (BGRA → NV12/P010) và thu phóng ngay trên GPU bằng ID3D11VideoProcessor::VideoProcessorBlt hoặc Compute Shader, sau đó đăng ký trực tiếp tài nguyên DirectX vào bộ mã hóa phần cứng (NvEncRegisterResource, AMFContext::CreateSurfaceFromDX11Native, oneVPL D3D11 surface hoặc D3D12 resource). Khi màn hình tĩnh (DXGI\_ERROR\_WAIT\_TIMEOUT hoặc LastPresentTime \== 0), bộ mã hóa không được gọi; nếu chỉ con trỏ chuột di chuyển, hệ thống lấy bitmap qua GetFramePointerShape() để Android vẽ cục bộ, giúp tải GPU mã hóa giảm về \~0% lúc đọc tài liệu.  
Với **đa màn hình và HDR**, tiến trình duyệt toàn bộ IDXGIOutput6 để đọc tọa độ ảo, mật độ điểm ảnh DPI và ColorSpace. Nếu màn hình bật HDR (DXGI\_COLOR\_SPACE\_RGB\_FULL\_G2084\_NONE\_P2020), ứng dụng bắt buộc dùng **IDXGIOutput5::DuplicateOutput1()** để nhận trực tiếp định dạng DXGI\_FORMAT\_R16G16B16A16\_FLOAT thay vì DuplicateOutput() vốn ép xuống 8-bit BGRA gây cháy sáng. Khi điện thoại Android chỉ hỗ trợ SDR, một shader GPU thực hiện **HDR-to-SDR Tone Mapping** (theo nguyên lý chuỗi CLSID\_D2D1HdrToneMap của Direct2D) ngay trước bước nén.  
Với **màn hình khóa, UAC và Secure Desktop**, Windows cô lập hộp thoại UAC và Winlogon trong desktop bảo mật chỉ cho phép tiến trình quyền SYSTEM truy cập. Ứng dụng triển khai một **Windows Service chạy ở Session 0 dưới tài khoản LocalSystem**, dùng WTSEnumerateSessions() / WTSGetActiveConsoleSessionId() để xác định phiên đang hoạt động và khởi chạy tiến trình worker quyền SYSTEM vào đúng session đó qua CreateProcessAsUser. Khi bắt lỗi DXGI\_ERROR\_ACCESS\_LOST do chuyển sang UAC, worker gọi OpenInputDesktop() \+ SetThreadDesktop() và khởi tạo lại IDXGIOutputDuplication trong \<50 ms. Đối với **máy không gắn màn hình (headless PC)**, hệ thống hỗ trợ cài đặt tùy chọn trình điều khiển màn hình ảo user-mode chuẩn **IddCx (Indirect Display Driver Model)** chạy ở Session 0, tạo bề mặt DirectX thật cho DWM kết xuất mà không gây màn hình xanh nếu driver lỗi, đồng thời cho phép sinh độ phân giải khớp tỷ lệ màn hình điện thoại/tablet Android.

### **Mã mẫu: Chụp màn hình DXGI Zero-Copy sang NVENC (C++/Win32)**

// Đường đi Zero-Copy VRAM: DXGI Desktop Duplication \-\> ID3D11Texture2D \-\> NVENC  
HRESULT CaptureAndEncodeZeroCopy(  
    IDXGIOutputDuplication\* dup, ID3D11DeviceContext\* ctx,  
    ID3D11Texture2D\* nvencInputTex, void\* nvEncSession,  
    NV\_ENCODE\_API\_FUNCTION\_LIST& nvApi, NV\_ENC\_REGISTERED\_PTR regRes) {

    DXGI\_OUTDUPL\_FRAME\_INFO frameInfo \= {};  
    ComPtr\<IDXGIResource\> desktopRes;  
    HRESULT hr \= dup-\>AcquireNextFrame(16, \&frameInfo, \&desktopRes);  
    if (hr \== DXGI\_ERROR\_WAIT\_TIMEOUT) return S\_FALSE; // Màn hình tĩnh \-\> nghỉ tiết kiệm điện  
    if (hr \== DXGI\_ERROR\_ACCESS\_LOST) return hr;       // Đổi độ phân giải / UAC \-\> re-init

    // Bỏ qua bước mã hóa video nếu chỉ con trỏ chuột di chuyển  
    if (frameInfo.LastPresentTime.QuadPart \== 0\) {  
        dup-\>ReleaseFrame();  
        return S\_FALSE;  
    }

    ComPtr\<ID3D11Texture2D\> acquiredTex;  
    desktopRes.As(\&acquiredTex);

    // Chép GPU-to-GPU trong VRAM (hoặc dùng VideoProcessorBlt BGRA-\>NV12 tại đây)  
    ctx-\>CopyResource(nvencInputTex, acquiredTex.Get());  
    dup-\>ReleaseFrame();

    // Map trực tiếp tài nguyên D3D11 đã đăng ký với NV\_ENC\_INPUT\_RESOURCE\_TYPE\_DIRECTX  
    NV\_ENC\_MAP\_INPUT\_RESOURCE mapParams \= { NV\_ENC\_MAP\_INPUT\_RESOURCE\_VER };  
    mapParams.registeredResource \= regRes;  
    if (nvApi.nvEncMapInputResource(nvEncSession, \&mapParams) \== NV\_ENC\_SUCCESS) {  
        NV\_ENC\_PIC\_PARAMS picParams \= { NV\_ENC\_PIC\_PARAMS\_VER };  
        picParams.inputBuffer    \= mapParams.mappedResource;  
        picParams.bufferFmt      \= mapParams.mappedBufferFmt;  
        picParams.pictureStruct  \= NV\_ENC\_PIC\_STRUCT\_FRAME;  
        picParams.inputTimeStamp \= frameInfo.LastPresentTime.QuadPart;  
        nvApi.nvEncEncodePicture(nvEncSession, \&picParams);  
        nvApi.nvEncUnmapInputResource(nvEncSession, mapParams.mappedResource);  
    }  
    return S\_OK;  
}

### **Rủi ro và giảm thiểu**

| Rủi ro chụp màn hình Windows | Cách giảm thiểu |
| :---- | :---- |
| Laptop Hybrid GPU (iGPU gắn màn hình, dGPU chạy NVENC) gây lỗi DuplicateOutput | Tạo thiết bị D3D11 trên đúng adapter sở hữu màn hình để nén bằng iGPU (QuickSync/VCN), hoặc tự chuyển sang WGC với shared texture. |
| Lỗi driver Intel Iris Xe làm sáng trắng ảnh chụp DXGI trên một số bản Windows 11 24H2 | Cho phép chuyển nhanh sang WGC trong cài đặt và ghi nhận cảnh báo kiểm tra cấu hình màu driver. |
| Tự đóng gói driver ảo IddCx yêu cầu chứng chỉ EV và quy trình ký WHCP/Attestation tốn phí | Cung cấp màn hình ảo IddCx dạng tùy chọn cài rời (liên kết tới driver mã nguồn mở cộng đồng đã ký WHQL) thay vì nhúng cưỡng bức vào bộ cài lõi. |

**Danh sách nguồn Agent 2:**

> * Microsoft Indirect Display Driver Model (IddCx): https\://learn.microsoft.com/en-us/windows-hardware/drivers/display/indirect-display-driver-model-overview  
> * Microsoft IDXGIOutputDuplication::GetFrameDirtyRects: https\://learn.microsoft.com/en-us/windows/win32/api/dxgi1\_2/nf-dxgi1\_2-idxgioutputduplication-getframedirtyrects  
> * So sánh DXGI và WGC (OBS & GStreamer): https\://obsproject.com/forum/threads/windows-graphics-capture-vs-dxgi-desktop-duplication.149320/ và https\://discourse.gstreamer.org/t/d3d11screencapturesrc-vs-d3d12screencapturesrc/2080/2  
> * Chụp HDR DXGI\_FORMAT\_R16G16B16A16\_FLOAT & Direct2D HDR Tone Map: https\://learn.microsoft.com/en-us/answers/questions/1457052/using-the-desktop-duplication-api-with-hdr-interpr và https\://learn.microsoft.com/da-dk/windows/win32/direct2d/hdr-tone-map-effect  
> * Khởi chạy tiến trình từ Session 0 Service & Secure Desktop: https\://stackoverflow.com/questions/26913172/create-process-in-user-session-from-service và https\://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-10/security/threat-protection/security-policy-settings/user-account-control-switch-to-the-secure-desktop-when-prompting-for-elevation

## **Agent 3 – Mã hóa video trên Windows và công thức thanh trượt chất lượng**

### **Phương án đề xuất**

Triển khai lớp trừu tượng mã hóa theo thứ tự ưu tiên 4 tầng: **(1) SDK phần cứng bản địa** (NVENC NVIDIA, AMF AMD, oneVPL Intel) trên Windows x64 để can thiệp sâu từng cờ độ trễ cực thấp (intra-refresh, VBV 1 khung, chia slice); **(2) D3D12 Video Encode API** cho Windows 11 24H2+ (WDDM 3.2) như nhánh thống nhất hỗ trợ cả AV1 Encode lẫn DirtyMap/QPMap trên GPU; **(3) Media Foundation Transforms (MFT)** làm backend chính trên **Windows ARM64 (Snapdragon X Elite/Plus)** để khai thác VPU Adreno 4K60 10-bit; và **(4) Phần mềm native** ưu tiên **OpenH264** tải nhị phân trực tiếp từ Cisco làm phương án dự phòng sạch bản quyền.

### **Bảng so sánh API mã hóa và các codec video**

| API mã hóa Windows (Agent 3a) | Trễ (25%) | Điện (20%) | Ảnh (15%) | Ổn định (12%) | Tương thích (10%) | Bảo trì (8%) | Đơn giản (5%) | Pháp lý (5%) | Tổng điểm |
| :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- |
| **SDK bản địa (NVENC+AMF+oneVPL) \+ MFT ARM64** | 5 | 5 | 5 | 5 | 5 | 3 | 2 | 5 | **4,69** |
| D3D12 Video Encode API (Thống nhất Win11+) | 5 | 5 | 4 | 4 | 3 | 5 | 3 | 5 | **4,43** |
| FFmpeg avcodec HW wrappers | 4 | 4 | 4 | 4 | 5 | 4 | 4 | 4 | **4,10** |
| Media Foundation Transforms (MFT thuần) | 3 | 4 | 3 | 4 | 5 | 5 | 4 | 5 | **3,83** |

| Codec Video (Agent 3b) | Trễ (25%) | Điện (20%) | Ảnh (15%) | Ổn định (12%) | Tương thích (10%) | Bảo trì (8%) | Đơn giản (5%) | Pháp lý (5%) | Tổng điểm |
| :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- |
| **H.264/AVC (OS/GPU HW \+ OpenH264 SW)** | 5 | 5 | 3 | 5 | 5 | 5 | 5 | 4 | **4,65** |
| **H.265/HEVC (Chỉ dùng OS/GPU HW)** | 5 | 5 | 5 | 4 | 4 | 4 | 4 | 3 | **4,55** |
| **AV1 (HW \+ SVT-AV1/dav1d SW)** | 4 | 4 | 5 | 4 | 3 | 5 | 3 | 5 | **4,13** |
| VP9 (HW \+ libvpx SW) | 3 | 3 | 4 | 4 | 4 | 3 | 4 | 5 | **3,52** |

*Đặc thù phần cứng 2025–2026 cần lưu ý:* NVIDIA Blackwell (RTX 50 series, Video Codec SDK 13.0) nâng cấp NVENC thế hệ 9 cải thiện 5% chất lượng HEVC/AV1 và bổ sung 4:2:2 10-bit H.264/HEVC. Với AMD, RDNA 3/4 hỗ trợ mã hóa AV1 trên dòng tầm trung–cao (nhưng các nhân phổ thông như RX 6400, RX 6500 XT và Navi 44 bị cắt bỏ khối mã hóa phần cứng), và ghi nhận từ cộng đồng người dùng OBS cho thấy chất lượng H.264 trên RX 9000 (RDNA 4\) đã tiệm cận NVIDIA. Trên Intel, một số GPU tích hợp mới như Arc 140T chỉ lộ bộ mã hóa phần cứng AV1 qua **oneVPL/QuickSync** chứ không đăng ký trong Media Foundation.

### **Giấy phép bằng sáng chế khi phân phối miễn phí**

Để dự án mã nguồn mở phát hành miễn phí không vướng kiện tụng bằng sáng chế, cần tuân thủ nguyên tắc phân tách phần cứng/phần mềm:

> * **H.264/AVC:** Bảo hộ qua Via Licensing Alliance (trước là MPEG LA). Khi gọi API phần cứng sẵn có của Windows/GPU và Android MediaCodec, phí bản quyền đã nằm trong thiết bị và hệ điều hành. Với nhánh dự phòng phần mềm, **không đóng gói sẵn x264** (vừa dính GPLv2 vừa không có miễn trừ bằng sáng chế khi phát hành nhị phân đại trà); thay vào đó tải động nhị phân **OpenH264 (BSD-2-Clause, v2.6.0)** trực tiếp từ máy chủ Cisco khi cần — Cisco chi trả phí bản quyền MPEG LA cho các bản tải trực tiếp này.  
> * **H.265/HEVC:** Rủi ro bằng sáng chế phức tạp nhất (Access Advance đã thâu tóm đơn vị quản trị bể HEVC/VVC của Via LA và gia hạn biểu phí đến năm 2030). Do đó, **chỉ kích hoạt HEVC qua bộ mã hóa/giải mã phần cứng hệ điều hành/GPU**, tuyệt đối không nhúng x265 trong bộ cài.  
> * **VP9 và AV1:** Định hướng miễn phí bản quyền (royalty-free) bởi Google và AOM, dù cần biết tổ chức Sisvel có vận hành một bể bằng sáng chế độc lập liên quan đến AV1. Chuẩn kế nhiệm **AV2** đã ra mắt ngày 28/05/2026, nhưng phần cứng giải mã điện thoại chưa phổ cập nên AV1 vẫn là lựa chọn tiên tiến thực tế giai đoạn 2026–2028.

### **Cấu hình mã hóa siêu độ trễ thấp và chi phí CPU phần mềm 1080p60**

Các cấu hình livestream phổ thông (B-frame, Lookahead, 2-pass) cộng thêm 60–150 ms độ trễ và phải tắt hoàn toàn. Thông số tối ưu cho NVENC / AMF / oneVPL:

| Tham số mã hóa | Cấu hình đề xuất | Tác dụng kỹ thuật |
| :---- | :---- | :---- |
| Tuning / Preset | NV\_ENC\_TUNING\_INFO\_ULTRA\_LOW\_LATENCY, Preset P1–P3 | Nghiên cứu benchmark 2025–2026 cho thấy chế độ Ultra Low-Latency cắt độ trễ rõ rệt trong khi chất lượng bộ mã hóa phần cứng ít nhạy với preset tốc độ. |
| B-Frames | 0 (frameIntervalP \= 1, chỉ dùng I/P-frame) | Loại bỏ độ trễ sắp xếp lại khung hình (16–66 ms ở 60 FPS). |
| Lookahead & Multipass | enableLookahead \= 0, multiPass \= DISABLED | Xuất bitstream ngay trong chu kỳ khung hình hiện tại (\<2–5 ms). |
| Intra-Refresh | intraRefreshEnableFlag \= 1, chu kỳ 120 khung, GOP vô hạn | Triệt tiêu đỉnh tăng vọt kích thước I-frame định kỳ gây nghẽn hàng đợi mạng; chỉ tạo IDR khi kết nối mới hoặc mất gói nặng. |
| Bộ đệm VBV (HRD) | vbvBufferSize \= targetBitrate / fps (1 khung), vbvInitialDelay \= vbvBufferSize | Giữ kích thước từng khung ổn định, chống dồn gói (burst jitter). |
| Chia lát (Slice Mode) | sliceMode \= 3, sliceModeData \= 4 (4 lát/khung) | Cho phép truyền sớm từng lát và cô lập lỗi mất gói trong 1/4 màn hình. |
| Rate Control | **CBR** trên Internet/Wi-Fi nhiễu; **Constrained VBR** trên LAN | CBR giữ độ trễ ổn định nhất; Constrained VBR tiết kiệm mạnh băng thông khi màn hình đứng yên. |

**Chi phí CPU của phương án phần mềm ở 1080p60:** Đánh giá triển khai máy trạm chỉ ra mã hóa phần mềm tiêu tốn **20–40% CPU** mỗi luồng và tăng thêm **30–100 ms độ trễ** so với phần cứng. Ở mức độ từng bộ mã hóa, tài liệu benchmark ghi nhận x264 trên CPU 8 nhân hiện đại đạt khoảng 120 fps ở 1080p (nên 1080p60 chiếm khoảng 25–40% CPU tùy preset), trong khi phản hồi thử nghiệm người dùng trên diễn đàn OBS với CPU Core Ultra 7 265KF cho thấy SVT-AV1 ở preset 9–10 vẫn ngốn **40–50% CPU**. Vì vậy, nhánh dự phòng phần mềm mặc định dùng **OpenH264** ở 720p30 hoặc 1080p30 và cảnh báo người dùng nếu tự nâng lên 1080p60.

### **Công thức ánh xạ thanh trượt chất lượng 1–100% sang Bitrate và QP/CRF**

Vì chất lượng cảm nhận (VMAF/SSIM) tỷ lệ logarit với bitrate và độ phức tạp tăng dưới tuyến tính theo độ phân giải và FPS, công thức phi tuyến dưới đây lấy mốc neo 1080p60 H.264 (từ 0,8 Mbps tại S \= 1 đến 45,0 Mbps tại S \= 100):  
s \= \\frac{S \- 1}{99}, \\qquad B\_{\\text{target}}(\\text{Mbps}) \= 0{,}8 \\cdot \\left(\\frac{45{,}0}{0{,}8}\\right)^{s^{0{,}90}} \\cdot \\left(\\frac{W \\cdot H}{1920 \\cdot 1080}\\right)^{0{,}75} \\cdot \\left(\\frac{\\text{FPS}}{60}\\right)^{0{,}55} \\cdot k\_{\\text{codec}} \\cdot k\_{\\text{content}} \\cdot k\_{\\text{chroma}}  
Các hệ số chuẩn hóa: hệ số hiệu quả codec thời gian thực k\_codec \= 1,00 (H.264), 0,72 (VP9), 0,65 (HEVC), 0,52 (AV1); hệ số nội dung k\_content \= 0,45 (văn phòng), 1,00 (hỗn hợp), 1,35 (game chuyển động cao); hệ số màu k\_chroma \= 1,45 nếu bật 4:4:4 (1,00 cho 4:2:0). Ánh xạ sang ngưỡng **QP/CRF mục tiêu** khi dùng chế độ VBR/CQP:  
\\text{QP}\_{\\text{H.264/HEVC}}(S) \= \\text{round}\\left(44 \- 28 \\cdot s^{0{,}85}\\right), \\qquad \\text{QP}\_{\\text{AV1/VP9}}(S) \= \\text{round}\\left(56 \- 38 \\cdot s^{0{,}85}\\right)  
Khi bật chế độ **tự động thích ứng băng thông (ABR)**, bitrate truyền thực tế bị chặn trần ở 85% băng thông khả dụng ước lượng từ tầng mạng (sàn tối thiểu 300 kbps). Nếu băng thông xuống thấp hơn ngưỡng sàn của hồ sơ hiện tại, hệ thống hạ FPS theo bậc (120 → 60 → 30\) trước khi hạ độ phân giải để giữ văn bản rõ nét.

### **Mã mẫu: Ánh xạ thanh trượt chất lượng sang Bitrate/QP và ABR (Rust)**

pub enum Codec { H264, Vp9, Hevc, Av1 }  
pub enum ContentType { Office, General, HighMotion }

pub struct EncodeTarget {  
    pub bitrate\_kbps: u32,  
    pub target\_qp: u8,  
    pub vbv\_size\_bits: u32,  
}

pub fn compute\_encode\_target(  
    slider\_pct: u8, width: u32, height: u32, fps: u32,  
    codec: Codec, content: ContentType, chroma\_444: bool,  
    estimated\_net\_kbps: Option\<u32\>,  
) \-\> EncodeTarget {  
    let s \= (slider\_pct.clamp(1, 100\) as f64 \- 1.0) / 99.0;  
    let b\_anchor\_mbps \= 0.8 \* (45.0\_f64 / 0.8).powf(s.powf(0.90));  
    let res\_f \= ((width as f64 \* height as f64) / (1920.0 \* 1080.0)).powf(0.75);  
    let fps\_f \= (fps.max(1) as f64 / 60.0).powf(0.55);

    let k\_codec \= match codec {  
        Codec::H264 \=\> 1.00, Codec::Vp9 \=\> 0.72, Codec::Hevc \=\> 0.65, Codec::Av1 \=\> 0.52,  
    };  
    let k\_content \= match content {  
        ContentType::Office \=\> 0.45, ContentType::General \=\> 1.00, ContentType::HighMotion \=\> 1.35,  
    };  
    let k\_chroma \= if chroma\_444 { 1.45 } else { 1.00 };

    let mut target\_kbps \= (b\_anchor\_mbps \* res\_f \* fps\_f \* k\_codec \* k\_content \* k\_chroma \* 1000.0) as u32;  
    if let Some(net\_kbps) \= estimated\_net\_kbps {  
        let net\_cap \= ((net\_kbps as f64) \* 0.85) as u32;  
        target\_kbps \= target\_kbps.min(net\_cap).max(300);  
    }

    let target\_qp \= match codec {  
        Codec::H264 \| Codec::Hevc \=\> (44.0 \- 28.0 \* s.powf(0.85)).round() as u8,  
        Codec::Vp9 \| Codec::Av1   \=\> (56.0 \- 38.0 \* s.powf(0.85)).round() as u8,  
    };  
    let vbv\_size\_bits \= (target\_kbps \* 1000\) / fps.max(1);  
    EncodeTarget { bitrate\_kbps: target\_kbps, target\_qp, vbv\_size\_bits }  
}

### **Rủi ro và giảm thiểu**

| Rủi ro mã hóa Windows | Cách giảm thiểu |
| :---- | :---- |
| Giới hạn tối đa 8 phiên NVENC đồng thời trên toàn hệ thống card GeForce không chuyên | Chỉ duy trì 1 phiên NVENC cho mỗi luồng kết nối và hủy sạch phiên ngay khi ngắt kết nối hoặc tạm dừng. |
| Phản ánh người dùng về lỗi không seek được tệp AV1 mã hóa trên Snapdragon X Elite | Với luồng live, luôn chèn lại OBU Sequence Header trên mỗi khung IDR và đặt HEVC MFT làm mặc định ổn định cho Snapdragon X thế hệ 1\. |
| Nghẽn GPU 3D khi chạy nén CPU phần mềm kết hợp lọc thu phóng nặng trên GPU | Khi chạy fallback phần mềm, dùng thu phóng CPU SIMD nhẹ, tách hẳn khỏi hàng đợi 3D GPU. |

**Danh sách nguồn Agent 3:**

> * NVIDIA Video Codec SDK 13.0 & NVENC Application Note: https\://developer.nvidia.com/blog/nvidia-video-codec-sdk-13-0-powered-by-nvidia-blackwell/ và https\://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/nvenc-application-note/index.html  
> * Microsoft D3D12 Video Encode API & Agility SDK 1.716.0: https\://learn.microsoft.com/en-us/windows-hardware/drivers/display/video-encoding-d3d12 và https\://devblogs.microsoft.com/directx/agility-sdk-1-716-0-new-d3d12-video-encode-features/  
> * Intel oneVPL & QSV benchmark: https\://www\.intel.com/content/www/us/en/developer/articles/guide/onevpl-installation-guide.html và https\://vajracast.com/blog/hardware-transcoding-intel-qsv/  
> * AMD RDNA 4 / VCN: https\://www\.igorslab.de/en/amd-rdna-4-av1-coding-and-restrictions-for-beginner-gpus/ và https\://en.wikipedia.org/wiki/Video\_Core\_Next  
> * Windows ARM64 Snapdragon X Elite MFT: https\://multicorewareinc.com/handbrake-windows-on-snapdragon-project/  
> * Benchmark 2026 & giấy phép OpenH264/HEVC: https\://www\.forasoft.com/learn/video-quality/articles-vqm/encoder-comparison-x264-x265-svt-av1 và https\://bloggeek.me/webrtcglossary/openh264/

## **Agent 4 – Giải mã và hiển thị độ trễ thấp trên Android**

### **Phương án đề xuất**

Kết hợp **lõi mạng/tách gói Rust NDK** đẩy trực tiếp byte buffer đã ghép khung vào **MediaCodec xuất thẳng ra SurfaceView** (zero-copy hardware surface thẳng tới SurfaceFlinger, tránh lớp trộn GPU thừa của TextureView). Vì Java MediaCodec và NDK AMediaCodec dùng chung một thư viện native bên dưới với tốc độ giải mã tương đương, việc dùng **Java/Kotlin MediaCodec** để truy vấn đầy đủ năng lực phần cứng (isHardwareAccelerated(), getSupportedVendorParameters()) và cấu hình thang hạ cấp (fallback ladder) sẽ an toàn hơn trên Android 8–10, trong khi trên Android 11+ có thể truyền thẳng sang **AMediaCodec (NDK)** với khóa "low-latency" nếu muốn toàn bộ luồng giải mã chạy trong Rust/C++. Dự phòng phần mềm dùng **dav1d** (cho AV1 khi cần) hoặc **FFmpeg** xuất trực tiếp ra ANativeWindow.

### **Bảng so sánh đường ống giải mã và hiển thị Android**

| Đường ống giải mã Android (Agent 4\) | Trễ (25%) | Điện (20%) | Ảnh (15%) | Ổn định (12%) | Tương thích (10%) | Bảo trì (8%) | Đơn giản (5%) | Pháp lý (5%) | Tổng điểm |
| :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- |
| **Java MediaCodec \+ Rust Demux → SurfaceView** | 5 | 5 | 5 | 5 | 5 | 5 | 4 | 5 | **4,95** |
| Thuần AMediaCodec (NDK) → SurfaceView | 5 | 5 | 5 | 4 | 5 | 4 | 3 | 5 | **4,70** |
| MediaCodec → TextureView (Compose/GL) | 3 | 3 | 5 | 5 | 5 | 5 | 5 | 5 | **4,10** |
| Phần mềm dav1d/FFmpeg → ANativeWindow | 2 | 1 | 5 | 5 | 5 | 4 | 4 | 5 | **3,32** |

### **Khóa độ trễ thấp theo hãng SoC và cơ sở dữ liệu lỗi phần cứng**

Mặc định, bộ giải mã Android có thể giữ lại 4–8 khung hình trong hàng đợi nội bộ để tối ưu điện năng, gây trễ 60–130 ms. Ứng dụng phải áp dụng thang cấu hình thử lần lượt từ khóa hãng xuống chuẩn cơ bản:

| Hãng SoC / Nền tảng | Khóa MediaFormat độ trễ thấp | Lỗi đã ghi nhận & Lưu ý (2025–2026) |
| :---- | :---- | :---- |
| **Chuẩn Android 11+ (API 30+)** | MediaFormat.KEY\_LOW\_LATENCY \= 1 ("low-latency"), KEY\_PRIORITY \= 0 | Kiểm tra trước qua isFeatureSupported(FEATURE\_LowLatency). Tài liệu Android lưu ý bật low-latency có thể tăng mức tiêu thụ điện. |
| **Qualcomm Snapdragon** | vendor.qti-ext-dec-low-latency.enable \= 1; vendor.qti-ext-dec-picture-order.enable \= 1; KEY\_OPERATING\_RATE \= Short.MAX\_VALUE | **Cấm đặt KEY\_OPERATING\_RATE** trên Snapdragon 765G (Adreno 620\) vì gây crash sau start(). AV1 HW có từ Snapdragon 8 Gen 2+ và 7 Gen 4 (05/2025). |
| **Samsung Exynos** | vendor.rtc-ext-dec-low-latency.enable \= 1; vendor.rtc-ext-dec-output-queue-depth.value \= 2; vendor.sec-dec-output.delay \= 0 | Không đặt KEY\_OPERATING\_RATE trên Exynos. Ghi nhận lỗi c2.exynos.hevc.decoder Error 0xe trên Galaxy S21/S24 với luồng HEVC trực tiếp — cần watchdog chuyển H.264. AV1 HW từ Exynos 2100/2200+. |
| **MediaTek Dimensity / Helio** | vendor.mtk.vdec.ultra-low-latency \= 1; vdec-lowlatency \= 1 | **Lỗi hồi quy Android 15:** Dimensity 700/900/1080 (OMX.MTK.HEVC.Decoder) mở thành công nhưng màn hình đen im lặng không sinh log lỗi. Cộng đồng phản ánh Helio G99/Dimensity 1300T có thể trễ decode 20–28 ms. |
| **Google Tensor** | KEY\_LOW\_LATENCY \= 1, KEY\_PRIORITY \= 0 | Hỗ trợ giải mã phần cứng AV1 và HEVC đầy đủ trên dòng Tensor. |
| **Unisoc / Amlogic (TV Box)** | Amlogic: vendor.low-latency.enable \= 1, vdec-lowlatency \= 1 (tránh bật cờ low-latency HEVC trên c2.amlogic ngoài Fire TV vì tụt xuống \~1 FPS) | Báo cáo kỹ thuật của Meta (2025) chỉ rõ unisoc.av1.decoder trên dòng T606 là biến thể phần mềm dav1d giả danh HW. Phải loại trừ khỏi danh sách HW AV1. |

### **Đồng bộ hiển thị, tiết kiệm pin và khóa Wi-Fi**

Trên màn hình 90/120 Hz, gọi Surface.setFrameRate(fps, FRAME\_RATE\_COMPATIBILITY\_FIXED\_SOURCE) (hoặc ANativeWindow\_setFrameRate trong NDK) để khớp tần số quét tấm nền với FPS nguồn. Khi màn hình Windows đứng yên quá 500 ms, tự hạ setFrameRate(10f, ...) để màn hình LTPO hạ nhịp quét tiết kiệm pin. Ở chế độ Game/Đồ họa, hiển thị khung hình ngay khi giải mã xong (releaseOutputBuffer(idx, System.nanoTime())) để cắt độ trễ Vsync; ở chế độ Xem video, đồng bộ theo callback **Choreographer** để chuyển động đều. Với mạng Wi-Fi, trên Android 10+ (API 29+) kích hoạt khóa WifiManager.WIFI\_MODE\_FULL\_LOW\_LATENCY (chỉ có hiệu lực khi màn hình bật và app ở tiền cảnh) để tránh chip Wi-Fi rơi vào chế độ tiết kiệm điện gây nhảy ping. Ngay khi ứng dụng vào onStop(), gửi lệnh Control::PauseVideo lên Windows để dừng hoàn toàn chụp và mã hóa.

### **Mã mẫu: Khởi tạo MediaCodec độ trễ thấp kèm Fallback Ladder (Kotlin & NDK)**

fun configureLowLatencyDecoder(  
    codecName: String, mime: String, width: Int, height: Int, surface: Surface, socVendor: String  
): MediaCodec {  
    val codec \= MediaCodec.createByCodecName(codecName)  
    // Thang hạ cấp 3 mức: (0) Vendor \+ API 30 \-\> (1) API 30 chuẩn \-\> (2) Cơ bản an toàn  
    // (Tương đương NDK: AMediaFormat\_setInt32(fmt, "low-latency", 1); AMediaCodec\_configure(...))  
    for (attempt in 0..2) {  
        val format \= MediaFormat.createVideoFormat(mime, width, height).apply {  
            if (attempt \<= 1 && Build.VERSION.SDK\_INT \>= Build.VERSION\_CODES.R) {  
                setInteger(MediaFormat.KEY\_LOW\_LATENCY, 1\)  
                setInteger(MediaFormat.KEY\_PRIORITY, 0\)  
            }  
            if (attempt \== 0\) {  
                when (socVendor) {  
                    "qti" \-\> {  
                        setInteger("vendor.qti-ext-dec-low-latency.enable", 1\)  
                        setInteger("vendor.qti-ext-dec-picture-order.enable", 1\)  
                        // Tránh đặt operating-rate trên SD 765G (Adreno 620 / nền tảng lito)  
                        if (\!Build.HARDWARE.contains("lito")) {  
                            setInteger(MediaFormat.KEY\_OPERATING\_RATE, Short.MAX\_VALUE.toInt())  
                        }  
                    }  
                    "exynos" \-\> {  
                        setInteger("vendor.rtc-ext-dec-low-latency.enable", 1\)  
                        setInteger("vendor.rtc-ext-dec-output-queue-depth.value", 2\)  
                        setInteger("vendor.sec-dec-output.delay", 0\)  
                    }  
                    "mtk" \-\> {  
                        setInteger("vendor.mtk.vdec.ultra-low-latency", 1\)  
                        setInteger("vdec-lowlatency", 1\)  
                    }  
                }  
            }  
        }  
        try {  
            codec.configure(format, surface, null, 0\)  
            codec.start()  
            return codec  
        } catch (\_: Exception) {  
            codec.reset()  
        }  
    }  
    throw IllegalStateException("Không thể khởi tạo MediaCodec cho \$codecName")  
}

### **Rủi ro và giảm thiểu**

| Rủi ro giải mã Android | Cách giảm thiểu |
| :---- | :---- |
| Bộ giải mã phần cứng treo im lặng ra màn hình đen (như MediaTek Dimensity Android 15 HEVC) | Cài đặt **No-Output Watchdog**: nếu đã nạp ≥15 khung nhưng sau 1.000 ms chưa có khung render, tự động ghi blacklist codec cục bộ và yêu cầu Windows hạ về H.264 ngay lập tức. |
| Chọn nhầm bộ giải mã phần mềm giả danh phần cứng (Unisoc unisoc.av1.decoder) gây hao pin mạnh | Lọc isHardwareAccelerated(), loại bỏ rõ c2.android.\*, OMX.google.\*, unisoc.av1.decoder, và đo thời gian giải mã 30 khung đầu — nếu trung bình \>12 ms tự chuyển H.264 HW. |

**Danh sách nguồn Agent 4:**

> * AOSP Low-latency decoding in MediaCodec: https\://source.android.com/docs/core/media/low-latency-media  
> * Android NDK Media (AMediaCodec) & Android 11 KEY\_LOW\_LATENCY: https\://developer.android.com/ndk/reference/group/media và https\://developer.android.com/about/versions/11/features  
> * Moonlight MediaCodecHelper.java & tài liệu cờ giải mã Nightjar: https\://github.com/moonlight-stream/moonlight-android/blob/master/app/src/main/java/com/limelight/binding/video/MediaCodecHelper.java và https\://github.com/RamazanKara/nightjar/blob/main/docs/decoder-flags.md  
> * Lỗi MediaTek Android 15 HEVC & Exynos HEVC: https\://issuetracker.google.com/issues/437738589 và https\://github.com/androidx/media/issues/1292  
> * Meta AV1 White Paper (2025): https\://engineering.fb.com/wp-content/uploads/2025/09/Meta-AV1-White-Paper-FINAL.pdf  
> * Android Surface.setFrameRate & WIFI\_MODE\_FULL\_LOW\_LATENCY: https\://developer.android.com/media/optimize/performance/frame-rate và https\://developer.android.com/sdk/api\_diff/29/changes/android.net.wifi.WifiManager

## **Agent 5 – Mạng truyền tải độ trễ thấp và xuyên NAT chi phí gần bằng không**

### **Phương án đề xuất**

Sử dụng **giao thức UDP tùy chỉnh trên nền Rust** kết hợp **sửa lỗi Reed-Solomon thích ứng (RS(n, k))**, điều khiển nghẽn dựa trên biến thiên độ trễ (kiểu **GCC/BBR**), kênh nhập liệu ưu tiên tuyệt đối (kèm gửi lặp RED khi mất gói), và **kênh dự phòng TCP/TLS cổng 443** cho mạng doanh nghiệp chặn UDP. Quy trình thiết lập kết nối không cần mở cổng router chạy song song 4 tầng: **(1) LAN mDNS** → **(2) IPv6 Direct** → **(3) STUN \+ UDP Hole Punching trên cùng một socket** → **(4) Máy chủ Relay tự dựng tối giản bằng Rust (hoặc coturn)**.

### **Bảng so sánh 6 giao thức truyền tải**

| Giao thức truyền tải (Agent 5\) | Trễ (25%) | Điện (20%) | Ảnh (15%) | Ổn định (12%) | Tương thích (10%) | Bảo trì (8%) | Đơn giản (5%) | Pháp lý (5%) | Tổng điểm |
| :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- |
| **UDP tùy chỉnh \+ Adaptive RS-FEC \+ GCC/BBR (+TCP/443)** | 5 | 5 | 5 | 5 | 5 | 4 | 2 | 5 | **4,77** |
| QUIC (Unreliable Datagrams \+ Streams) | 4 | 4 | 4 | 4 | 4 | 5 | 4 | 5 | **4,13** |
| WebRTC (ICE \+ RTP/RTCP \+ DTLS-SRTP) | 4 | 3 | 4 | 5 | 5 | 4 | 4 | 5 | **4,07** |
| KCP (Fast ARQ trên UDP \+ FEC) | 3 | 3 | 4 | 4 | 4 | 4 | 4 | 5 | **3,60** |
| SRT (Bounded ARQ) | 2 | 3 | 5 | 5 | 4 | 4 | 4 | 4 | **3,57** |
| TCP \+ TLS 1.3 | 1 | 3 | 4 | 5 | 5 | 5 | 5 | 5 | **3,45** |

Trong 6 lựa chọn, **TCP** mắc kẹt đầu hàng (head-of-line blocking) khi rớt gói; **SRT** mạnh về phát sóng nhưng vùng trễ tối ưu quanh 200–500 ms; **KCP** giảm trễ hơn TCP nhưng vẫn ép thứ tự byte và tăng lưu lượng thừa; **WebRTC** ổn định nhưng ngăn xếp libwebrtc nặng và khó tùy biến sâu đường ống intra-refresh; **QUIC** rất hấp dẫn nhờ chuyển mạng liền mạch và TLS 1.3 tích hợp, nhưng các nghiên cứu gần đây ghi nhận chi phí CPU di động cao hơn UDP nhẹ và tỷ lệ mất byte trên Wi-Fi nhạy cảm với gói đến sai thứ tự. Do đó, **UDP thuần bọc Noise E2EE \+ Adaptive RS-FEC** đạt hiệu suất tốt nhất cho mục tiêu \<30 ms và tiết kiệm pin.

### **Cơ chế xuyên NAT, sửa lỗi FEC và mô hình chi phí gần bằng 0**

Trong mạng nội bộ, Windows quảng bá dịch vụ \_remotedesk.\_udp.local. bằng API chuẩn **DnsServiceRegister** và Android tự phát hiện qua **NsdManager** để nối thẳng LAN (RTT \<2 ms). Qua Internet, dù các nhà mạng Việt Nam áp dụng **CGNAT IPv4** (hoạt động như Symmetric NAT làm hỏng UDP hole punching IPv4), tỷ lệ triển khai **IPv6** tại Việt Nam Q1/2026 đã vượt **45%** (FPT 52%, Viettel 48%, VNPT 41%) và hướng tới **72%** trong năm 2026 theo Quyết định 3369/QĐ-BKHCN. Việc ưu tiên thử **IPv6 Direct** (đục lỗ tường lửa stateful) trước khi dùng Relay giúp phần lớn kết nối giữa hai thuê bao Việt Nam đi thẳng P2P với chi phí máy chủ bằng 0\. Với IPv4 không bị CGNAT, hai đầu truy vấn STUN miễn phí (như stun.cloudflare.com:3478) trên **chính socket UDP truyền media** để khắc phục lỗi lệch IP egress TCP/UDP, đạt tỷ lệ xuyên NAT 82–95% trên router gia đình và duy trì bảng NAT bằng gói keepalive 32 byte mỗi 25 giây.  
Để không lãng phí băng thông, **Reed-Solomon FEC (RS(k \+ m, k))** chỉ bật khi mất gói \>0,5% và RTT \>25 ms (vì trên LAN RTT \<20 ms, gửi lại nhanh NACK hiệu quả hơn FEC). Kích thước gói giữ dưới 1.200 byte nhằm tránh phân mảnh MTU trên đường truyền ISP/4G. Gói nhập liệu (\<40 byte) được gắn ưu tiên cao nhất và gửi lặp RED 2 lần cách nhau 5 ms khi có mất gói để phản hồi chuột/phím không bị khựng.  
Về chi phí hạ tầng, dịch vụ Signaling (tương tự hbbs) chỉ truyền vài kilobyte mỗi lần bắt tay nên gần như miễn phí. Chi phí chỉ phát sinh ở băng thông chuyển tiếp (Relay) cho \~15% phiên rơi vào Symmetric NAT hoặc bị chặn UDP. Theo bảng giá năm 2026, dịch vụ TURN quản lý tốn **0,05 USD/GB** trên Cloudflare (sau 1.000 GB miễn phí), **0,40–0,80 USD/GB** trên Twilio và **0,05–0,09 USD/GB** tiền egress trên AWS. Khi thay bằng máy chủ Relay tự dựng trên VPS băng thông lớn (ước tính theo mức giá tham khảo thị trường như Oracle Cloud Always Free 10 TB/tháng hoặc Hetzner CX22 \~4,50 USD/tháng gồm 20 TB — lưu ý cần kiểm tra biểu phí cụ thể tại vùng Singapore/APAC khi triển khai thực tế để giữ RTT từ Việt Nam ở mức 25–45 ms), chi phí giảm gấp hơn 100 lần ở quy mô lớn:

### **Mã mẫu: Tính số mảnh Reed-Solomon FEC thích ứng và gửi ưu tiên nhập liệu (Rust)**

// Tính số mảnh parity Reed-Solomon theo tỷ lệ mất gói và RTT (tắt FEC trên LAN RTT \< 25 ms)  
pub fn adaptive\_fec\_parity\_shards(data\_shards\_k: usize, loss\_rate: f32, rtt\_ms: u32) \-\> usize {  
    if rtt\_ms \< 25 \|\| loss\_rate \< 0.005 \|\| data\_shards\_k \== 0 {  
   \[span\_29\](start\_span)\[span\_29\](end\_span)     return 0; // Trên LAN, NACK/khung tiếp theo hiệu quả hơn FEC  
    }  
    let ratio \= (loss\_rate \* 1.5).clamp(0.05, 0.25);  
    ((data\_shards\_k as f32) \* ratio).ceil() as usize  
}

// Gửi gói nhập liệu ưu tiên cao kèm RED (gửi lặp 1 bản khi mạng mất gói \> 1%)  
pub async fn send\_priority\_input(sock: \&tokio::net::UdpSocket, peer: std::net::SocketAddr, pkt: &\[u8\], loss\_rate: f32) {  
    let \_ \= sock.send\_to(pkt, peer).await;  
    if loss\_rate \> 0.01 {  
        tokio::time::sleep(std::time::Duration::from\_millis(5)).await;  
        let \_ \= sock.send\_to(pkt, peer).await;  
    }  
}

### **Rủi ro và giảm thiểu**

| Rủi ro mạng & truyền tải | Cách giảm thiểu |
| :---- | :---- |
| Tường lửa công ty chặn hoàn toàn UDP | Tự động rơi về đường hầm **WebSocket/TLS trên cổng TCP 443** qua máy chủ Relay để hòa lẫn lưu lượng HTTPS thông thường. |
| Người dùng di chuyển chuyển từ Wi-Fi sang 5G làm đổi IP nguồn | Đính kèm session\_id 32-bit vào đầu khung UDP; máy Windows tự cập nhật endpoint mới ngay khi xác thực thành công thẻ AEAD của gói tin đầu tiên từ IP mới. |
| Máy chủ Relay cộng đồng bị lợi dụng làm proxy lậu | Khóa quyền truy cập Relay bằng khóa công khai Ed25519 (kiểu hbbr \-k) và áp trần 10 Mbps cho mỗi luồng đi qua Relay. |

**Danh sách nguồn Agent 5:**

> * STUN/TURN/ICE & bảng giá 2026: https\://www\.forasoft.com/learn/video-streaming/articles-streaming/nat-stun-turn-ice-webrtc  
> * Tự dựng máy chủ Relay hbbs/hbbr và chi phí egress: https\://www\.ssdnodes.com/learn/self-host-rustdesk-relay-server  
> * Sửa lỗi UDP hole punching lệch IP egress trên RustDesk: https\://github.com/rustdesk/rustdesk/discussions/16488  
> * Thống kê IPv6 Việt Nam 2026 & Quyết định 3369/QĐ-BKHCN: https\://dptcloud.vn/en/blog/ipv6-dual-stack-bgp-routing-vietnam-2026 và https\://thuvienphapluat.vn/van-ban/Cong-nghe-thong-tin/Quyet-dinh-3369-QD-BKHCN-2025-chuyen-doi-IPv6-only-cho-Viet-Nam-giai-doan-2026-2030-678998.aspx  
> * Phân tích FEC vs Retransmission & QUIC vs WebRTC: https\://moq.dev/blog/forward-error-correction và https\://www\.naevatec.com/blog/quic-vs-tcp-udp-webrtc-media-transport  
> * Android NsdManager & Windows DnsServiceRegister: https\://developer.android.com/develop/connectivity/wifi/use-nsd và https\://stackoverflow.com/questions/75529457/android-nsd-cant-resolve-the-mdns-service-in-windows

## **Agent 6 – Nhập liệu đa phương thức và trải nghiệm điều khiển**

### **Phương án đề xuất**

Xây dựng kiến trúc nhập liệu hai nhánh trên Windows (chạy trong tiến trình quyền SYSTEM để hoạt động cả trên UAC/Secure Desktop) kết hợp đầy đủ API hiện đại của Android:

> * **Phía Windows:** Dùng **SendInput** với KEYEVENTF\_SCANCODE cho bàn phím vật lý, phím tắt (Win, F1–F12, tổ hợp) và game; dùng KEYEVENTF\_UNICODE (chuỗi UTF-16 VK\_PACKET) cho văn bản tiếng Việt từ bàn phím ảo Android; dùng **InjectTouchInput** (POINTER\_TOUCH\_INFO) và **InjectSyntheticPointerInput** (POINTER\_PEN\_INFO) cho cảm ứng đa điểm và bút có lực nhấn/độ nghiêng; và gọi **SendSAS(FALSE)** từ sas.dll để gửi Ctrl+Alt+Del.  
> * **Phía Android:** Hỗ trợ song song chế độ **Trackpad tương đối** (mặc định điện thoại) và **Chạm trực tiếp tuyệt đối** (mặc định máy tính bảng), bắt chuột Bluetooth/OTG bằng **View.requestPointerCapture()**, giảm trễ bút vẽ bằng **requestUnbufferedDispatch()**, cung cấp thanh phím đặc biệt nổi và **vẽ con trỏ chuột cục bộ** ở tần số quét màn hình Android.

### **Bảng so sánh phương án gõ tiếng Việt và bàn phím**

| Phương án nhập tiếng Việt & Phím (Agent 6\) | Trễ (25%) | Điện (20%) | Ảnh (15%) | Ổn định (12%) | Tương thích (10%) | Bảo trì (8%) | Đơn giản (5%) | Pháp lý (5%) | Tổng điểm |
| :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- |
| **Lai (Hybrid): KEYEVENTF\_UNICODE cho IME ảo \+ ScanCode cho phím cứng/tắt** | 5 | 5 | 5 | 5 | 5 | 5 | 4 | 5 | **4,95** |
| Chỉ gửi ScanCode thô (phụ thuộc UniKey/EVKey trên Windows) | 5 | 5 | 5 | 3 | 3 | 4 | 5 | 5 | **4,46** |
| Đồng bộ qua Clipboard rồi tự động gửi Ctrl+V | 2 | 5 | 5 | 2 | 4 | 4 | 4 | 5 | **3,64** |

Cơ chế lai giải quyết triệt để xung đột dấu tiếng Việt: khi người dùng gõ trên bàn phím ảo Android (Gboard/Laban Key), chuỗi tiếng Việt đã bỏ dấu hoàn chỉnh từ InputConnection.commitText được gửi dưới dạng UTF-8 và tiêm bằng KEYEVENTF\_UNICODE — chạy đúng ngay cả khi máy Windows tắt UniKey. Khi dùng bàn phím Bluetooth/OTG hoặc bấm tổ hợp phím tắt, hệ thống gửi thẳng **Hardware Scan Code** để bộ gõ UniKey/EVKey và phím tắt Windows vận hành tự nhiên.

### **Cử chỉ, chuột ngoài, bút cảm ứng, con trỏ cục bộ và Clipboard**

Trên màn hình cảm ứng, bộ cử chỉ được chuẩn hóa dễ nhớ: chạm 1 ngón \= nhấp trái; giữ 400 ms hoặc chạm 2 ngón \= nhấp phải; rê 1 ngón \= di chuyển con trỏ có gia tốc mượt (chế độ Trackpad); chạm đúp giữ \= kéo thả; vuốt 2 ngón \= cuộn dọc/ngang; chụm/mở 2 ngón \= phóng to cục bộ khung hình Android (hoặc gửi Ctrl \+ Scroll lên Windows). Khi cắm chuột Bluetooth/OTG trên Android 8.0+ (API 26+), ứng dụng gọi surfaceView.requestPointerCapture() và đọc delta trực tiếp trong onCapturedPointerEvent(MotionEvent) để ẩn con trỏ Android gốc và cho phép xoay 360° trong phần mềm 3D/game (hỗ trợ thêm POINTER\_CAPTURE\_MODE\_RELATIVE trên API 37+).  
Với bút cảm ứng (S-Pen/Stylus), Android gọi view.requestUnbufferedDispatch(event) để không gom lô sự kiện và trích xuất getPressure(), AXIS\_TILT, AXIS\_ORIENTATION ánh xạ sang POINTER\_PEN\_INFO (lực nhấn 0–1024, góc nghiêng \-90° đến \+90°); lưu ý Windows chặn tiêm sự kiện bút tại màn hình Secure Desktop/Ctrl+Alt+Del vì bảo mật hệ điều hành, khi đó ứng dụng tự hạ về sự kiện chuột thường. Con trỏ chuột được vẽ cục bộ ngay trên lớp phủ Android mỗi khi Windows gửi thay đổi hình dạng con trỏ, triệt tiêu cảm giác "chuột bị kéo lê" trên mạng WAN. Clipboard hai chiều đồng bộ văn bản/ảnh nhỏ thông qua AddClipboardFormatListener (Windows user-session helper) và ClipboardManager (Android khi cửa sổ có focus).

### **Mã mẫu: SendInput Unicode, Touch Injection đa điểm và Pen Injection (C++/Win32)**

// 1\. Tiêm văn bản tiếng Việt Unicode (UTF-16) qua SendInput  
void InjectUnicodeString(const std::wstring& text) {  
    std::vector\<INPUT\> inputs;  
    inputs.reserve(text.size() \* 2);  
    for (wchar\_t ch : text) {  
        INPUT down \= {}, up \= {};  
        down.type \= INPUT\_KEYBOARD;  
        down.ki.wScan \= ch;  
        down.ki.dwFlags \= KEYEVENTF\_UNICODE;  
        up \= down;  
        up.ki.dwFlags \= KEYEVENTF\_UNICODE \| KEYEVENTF\_KEYUP;  
        inputs.push\_back(down);  
        inputs.push\_back(up);  
    }  
    if (\!inputs.empty()) SendInput((UINT)inputs.size(), inputs.data(), sizeof(INPUT));  
}

// 2\. Tiêm cảm ứng đa điểm qua InjectTouchInput (cần gọi InitializeTouchInjection(10, TOUCH\_FEEDBACK\_DEFAULT) lúc khởi tạo)  
void InjectMultiTouch(const std::vector\<std::tuple\<uint32\_t, int32\_t, int32\_t, uint32\_t\>\>& contacts) {  
    std::vector\<POINTER\_TOUCH\_INFO\> pts(contacts.size());  
    for (size\_t i \= 0; i \< contacts.size(); \++i) {  
        auto \[id, x, y, flags\] \= contacts\[i\];  
        pts\[i\].pointerInfo.pointerType     \= PT\_TOUCH;  
        pts\[i\].pointerInfo.pointerId       \= id;  
        pts\[i\].pointerInfo.ptPixelLocation \= { x, y };  
        pts\[i\].pointerInfo.pointerFlags    \= flags; // vd: POINTER\_FLAG\_DOWN \| INRANGE \| INCONTACT  
        pts\[i\].touchMask \= TOUCH\_MASK\_CONTACTAREA \| TOUCH\_MASK\_PRESSURE;  
        pts\[i\].rcContact \= { x \- 4, y \- 4, x \+ 4, y \+ 4 };  
        pts\[i\].pressure  \= 512;  
    }  
    if (\!pts.empty()) InjectTouchInput((UINT)pts.size(), pts.data());  
}

// 3\. Tiêm bút cảm ứng có lực nhấn (0..1024) và độ nghiêng \+ Gửi Ctrl+Alt+Del (SendSAS)  
void InjectStylusPoint(HSYNTHETICPOINTERDEVICE penDev, int32\_t x, int32\_t y,  
                       uint32\_t pressure, int32\_t tiltX, int32\_t tiltY, bool inContact) {  
    POINTER\_TYPE\_INFO pti \= {};  
    pti.type \= PT\_PEN;  
    pti.penInfo.pointerInfo.pointerType     \= PT\_PEN;  
    pti.penInfo.pointerInfo.pointerId       \= 1;  
    pti.penInfo.pointerInfo.ptPixelLocation \= { x, y };  
    pti.penInfo.pointerInfo.pointerFlags    \= inContact  
        ? (POINTER\_FLAG\_INRANGE \| POINTER\_FLAG\_INCONTACT \| POINTER\_FLAG\_UPDATE)  
        : (POINTER\_FLAG\_INRANGE \| POINTER\_FLAG\_UPDATE);  
    pti.penInfo.penMask  \= PEN\_MASK\_PRESSURE \| PEN\_MASK\_TILT\_X \| PEN\_MASK\_TILT\_Y;  
    pti.penInfo.pressure \= pressure;  
    pti.penInfo.tiltX    \= tiltX;  
    pti.penInfo.tiltY    \= tiltY;  
    InjectSyntheticPointerInput(penDev, \&pti, 1);  
}  
// Ctrl+Alt+Del từ Windows Service quyền SYSTEM (cần bật chính sách SoftwareSASGeneration):  
// SendSAS(FALSE);

### **Rủi ro và giảm thiểu**

| Rủi ro nhập liệu | Cách giảm thiểu |
| :---- | :---- |
| Kẹt phím Ctrl/Alt/Shift/Win nếu rớt mạng đột ngột | Lưu trạng thái các phím đang giữ phía Windows; tự gửi KEYUP xả toàn bộ phím sau 500 ms mất tín hiệu hoặc khi đóng phiên. |
| Phím Backspace trên bàn phím ảo Android không kích hoạt khi ô nhập rỗng | Dùng EditText ẩn chứa sẵn ký tự đệm không hiển thị để luôn bắt sự kiện xóa và gửi VK\_BACK. |
| Lệnh SendSAS(FALSE) bị hệ điều hành từ chối | Trình cài đặt MSI cấu hình khóa chính sách HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\System\\SoftwareSASGeneration cho phép dịch vụ hệ thống gửi SAS. |

**Danh sách nguồn Agent 6:**

> * Microsoft SendInput & KEYBDINPUT: https\://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-keybdinput  
> * Microsoft InjectTouchInput & InjectSyntheticPointerInput: https\://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-injecttouchinput và https\://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-injectsyntheticpointerinput  
> * Microsoft SendSAS (sas.dll): https\://learn.microsoft.com/en-us/windows/win32/api/sas/nf-sas-sendsas  
> * Android requestPointerCapture & Stylus requestUnbufferedDispatch: https\://developer.android.com/develop/ui/views/touch-and-input/gestures/movement và https\://developer.android.com/develop/ui/compose/touch-input/stylus-input/advanced-stylus-features?hl=vi  
> * UniKey Vietnamese Input Engine: https\://www\.unikey.org/en/

## **Agent 7 – Bảo mật đầu cuối và chống xâm nhập**

### **Phương án đề xuất**

Sử dụng **Noise Protocol Framework** (Noise\_XX\_25519\_ChaChaPoly\_BLAKE2s cho lần kết nối đầu và Noise\_IK cho thiết bị đã ghim khóa, tùy chọn AES-256-GCM trên CPU có AES-NI/ARMv8 Crypto) làm lớp mã hóa đầu cuối (E2EE) cho mọi gói UDP video, âm thanh và nhập liệu. Kênh báo hiệu và dự phòng TCP/443 dùng **TLS 1.3** chuẩn. Xác thực phiên kết hợp **khóa công khai Ed25519**, **OTP một lần bảo vệ bằng giao thức PAKE (SPAKE2)** (hoặc mã QR chứa trực tiếp khóa công khai), **xác nhận thủ công bắt buộc trên màn hình Windows** và máy chủ trung gian hoàn toàn **zero-knowledge**.

### **Bảng so sánh 3 giao thức bảo mật đầu cuối**

| Giao thức bảo mật E2EE (Agent 7\) | Trễ (25%) | Điện (20%) | Ảnh (15%) | Ổn định (12%) | Tương thích (10%) | Bảo trì (8%) | Đơn giản (5%) | Pháp lý (5%) | Tổng điểm |
| :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- |
| **Noise Protocol (Noise\_XX/IK \+ X25519 \+ ChaCha20/AES-GCM)** | 5 | 5 | 5 | 5 | 5 | 5 | 4 | 5 | **4,95** |
| DTLS 1.3 \+ SRTP (RFC 9147 / RFC 5764\) | 4 | 4 | 5 | 5 | 5 | 4 | 3 | 5 | **4,37** |
| TLS 1.3 (Kênh dự phòng TCP & Signaling) | 3 | 4 | 5 | 5 | 5 | 5 | 5 | 5 | **4,30** |

**Noise Protocol** hoàn tất trao đổi khóa gọn nhẹ (\~100 byte, không mang gánh nặng phân tích chứng chỉ X.509/ASN.1 của DTLS), cài đặt gọn trong \~1.000 dòng mã Rust an toàn bộ nhớ (noise-protocol 0.2.1 \+ noise-rust-crypto 0.6.2) và bảo đảm bí mật chuyển tiếp hoàn hảo (Forward Secrecy). Lưu ý hạn chế đã được chỉ ra: Noise thuần dựa trên Diffie-Hellman phi tương tác nên chưa có sẵn cơ chế hậu lượng tử (Post-Quantum KEM) như TLS 1.3 hybrid; lộ trình v1.5 sẽ bổ sung trao đổi khóa lai ML-KEM trước phiên Noise để chống nguy cơ thu thập trước – giải mã sau (HNDL).

### **Xác thực, chống dò mật khẩu và máy chủ Zero-Knowledge**

Mỗi máy Windows sinh cặp khóa định danh dài hạn **Ed25519** ngay lần chạy đầu. Phiên mới tạo OTP ngẫu nhiên 6–8 ký tự (hết hạn sau 15 phút hoặc sau 1 lần dùng); hai đầu xác thực OTP bằng **SPAKE2** bên trong hầm Noise để mật khẩu không bao giờ bị lộ dưới dạng bản rõ hay bị tấn công từ điển ngoại tuyến. Nếu quét **mã QR**, Android nhận trực tiếp dấu vân tay khóa công khai của Host, loại bỏ hoàn toàn nguy cơ máy chủ Signaling bị chiếm quyền tráo khóa (MITM). Phía Windows giới hạn tốc độ chống dò mật khẩu cục bộ (sai 3 lần khóa 30 giây, sai 5 lần khóa 5 phút và hủy OTP cũ; mật khẩu cố định tùy chọn băm bằng **Argon2id**). Mọi phiên mới đều bật hộp thoại xác nhận nổi trên Windows hiển thị tên điện thoại Android và **6 ký tự Fingerprint**, yêu cầu bấm **"Cho phép"**. Máy chủ Signaling/Relay chỉ giữ bảng định tuyến ID tạm thời trong RAM (TTL 60 giây), không lưu mật khẩu, không lưu lịch sử phiên và không có khóa giải mã.

### **Mã mẫu: Khởi tạo phiên E2EE với noise-protocol (Rust)**

use noise\_protocol::{HandshakeState, patterns::noise\_xx};  
use noise\_rust\_crypto::{X25519, ChaCha20Poly1305, Blake2s};

// Khởi tạo Responder trên Windows Host (Noise\_XX cung cấp Mutual Auth \+ Forward Secrecy)  
pub fn build\_noise\_responder(  
    \[span\_226\](start\_span)\[span\_226\](end\_span)host\_static\_privkey: \[u8; 32\],  
    protocol\_prologue: &\[u8\],  
) \-\> HandshakeState\<X25519, ChaCha20Poly1305, Blake2s\> {  
    HandshakeState::new(  
        noise\_xx(),  
        false, // false \= Responder (Windows Host), true \= Initiator (Android)  
        protocol\_prologue,  
        Some(host\_static\_privkey),  
        None, None, None,  
    )  
}  
// Sau 3 thông điệp bắt tay, cặp CipherState (tx, rx) mã hóa từng gói UDP với 64-bit nonce  
// đi kèm cửa sổ trượt 64-bit chống tấn công phát lại (Anti-Replay Sliding Window).

### **Rủi ro và giảm thiểu**

| Rủi ro bảo mật | Cách giảm thiểu |
| :---- | :---- |
| Máy chủ Signaling bị tấn công và tráo khóa công khai (MITM) | Áp dụng TOFU (ghim khóa sau lần đầu) và cảnh báo đỏ lớn nếu khóa Host thay đổi; ưu tiên ghép đôi qua mã QR chứa hash khóa công khai. |
| Khai thác lỗ hổng giải mã gói mạng để chiếm quyền SYSTEM Windows | Tách tiến trình mạng chạy dưới tài khoản LOCAL\_SERVICE đặc quyền thấp trong sandbox, chỉ gửi thông điệp đã kiểm tra lược đồ qua Named Pipe sang tiến trình SYSTEM. |

**Danh sách nguồn Agent 7:**

> * So sánh TLS 1.3 và Noise Protocol (wolfSSL): https\://www\.wolfssl.com/tls-1-3-versus-noise-protocol/  
> * DTLS 1.3 (RFC 9147\) & SRTP: https\://www\.forasoft.com/learn/video-streaming/articles-streaming/dtls-srtp-tls-encryption-media và https\://pion.ly/blog/dtls-one-three/  
> * Thư viện Rust noise-protocol 0.2.1 & noise-rust-crypto 0.6.2: https\://lib.rs/crates/noise-protocol và https\://docs.rs/noise-rust-crypto  
> * Microsoft RDP Post-Quantum Hybrid TLS 1.3: https\://techcommunity.microsoft.com/blog/post-quantum-crypto-tech-blog/remote-desktop-protocol-and-pqc-tls-1-3-hybrid/4562178  
> * Xác thực khóa Ed25519 và chống lừa đảo (HelpBuoy): https\://www\.tqdev.com/2026-helpbuoy-secure-self-hosted-remote-support/

## **Agent 8 – Đo lường hiệu năng, năng lượng và bảng chuẩn mực**

### **Phương án đo lường độ trễ màn hình-đến-màn hình và năng lượng**

Không dùng đồng hồ phần mềm nội bộ để công bố độ trễ vì chúng bỏ qua hàng đợi DWM, độ trễ ẩn của bộ giải mã phần cứng và thời gian quét tấm nền. Quy trình kiểm chuẩn chuẩn hóa gồm **Cảm biến quang (Photodiode \+ vi điều khiển)** cho phòng lab CI hồi quy độ chính xác \<0,2 ms, **Camera tốc độ cao (240–960 FPS)** đặt chung hai màn hình đo tối thiểu 500 mẫu lấy trung vị (p50) và phân vị 95 (p95), kết hợp bộ công cụ soi sâu từng khâu: **PresentMon** và **GPUView \+ ETW** (Log.cmd → Merged.etl) trên Windows cùng **Perfetto (Battery Counters µA/µAh)** và **Battery Historian** trên Android.

### **Bảng so sánh 3 phương pháp đo độ trễ và năng lượng**

\| Phương pháp đo lường (Agent 8\) \| Trễ (25%) \| Điện (20%) \| Ảnh (15%) \| Ổn định (12%) \| Tương thích (10%) \| Bảo trì (8%) \| Đơn giản (5%) \| Pháp lý (5%) \| Tổng điểm \| \| :--- \| :---: \| :---: \| :---: \| :---: \| :---: \| :---: \| :---: \| :---: \| :---: \| \| **Cảm biến quang Photodiode \+ MCU \+ Perfetto/ETW** \| 5 \| 5 \| 5 \| 5 \| 5 \| 5 \| 3 \| 5 \| **4,90** \| \| Camera tốc độ cao (240–960 FPS) \+ Battery Historian \| 5 \| 4 \| 5 \| 4 \| 5 \| 4 \| 4 \| 5 \| **4,55** \| \| Chỉ đo bằng dấu thời gian phần mềm (Internal Timestamp) \| 2 \| 3 \| 3 \| 4 \| 5 \| 5 \| 5 \| 5 \| **3,43** \|

### **Bảng mục tiêu kỹ thuật theo hồ sơ (720p30 đến 4K60)**

*Ghi chú mức độ chắc chắn:* Bitrate được tính từ mô hình thanh trượt ở mức 70–75%; các chỉ số độ trễ và công suất là **mục tiêu kỹ thuật thiết kế ước lượng** (chưa đo thực nghiệm trên bản dựng cuối), tham chiếu theo dữ liệu công khai của Intel QSV, AMD VCN và bài đo Moonlight/Android.

| Hồ sơ | Bitrate mục tiêu (HEVC / AV1 — H.264) | Độ trễ LAN (p50 / p95) | Độ trễ WAN (RTT 25 ms) | Tải CPU / VPU Win (HW) | Công suất bổ sung Win | Công suất Android (HW Decode \+ Wi-Fi, chưa tính màn hình) |
| :---- | :---- | :---- | :---- | :---- | :---- | :---- |
| **720p30** | 1,3–3,5 Mbps (H.264: 2,5–6,6) | 18–26 ms / \<35 ms | 45–60 ms | \<2% CPU / 8–12% VPU | \+1,5–2,5 W (ước lượng) | \~0,5–0,8 W (mục tiêu) |
| **1080p60** | 4,2–11,5 Mbps (H.264: 6,8–17,8) | 14–22 ms / \<30 ms | 40–55 ms | 2–4% CPU / 15–25% VPU | \+2,5–4,0 W (ước lượng) | \~0,9–1,4 W (mục tiêu) |
| **1440p120** | 20,8–26,0 Mbps (H.264: 40,0) | 10–16 ms / \<24 ms | 35–50 ms | 4–6% CPU / 35–50% VPU | \+4,0–7,0 W (ước lượng) | \~1,8–2,6 W (mục tiêu) |
| **4K60** | 26,1–32,7 Mbps (H.264: 50,3) | 16–25 ms / \<35 ms | 45–65 ms | 4–6% CPU / 40–60% VPU | \+3,0–6,0 W | \~2,0–3,0 W (mục tiêu) |

### **Bảng đối chiếu với các giải pháp điều khiển từ xa hiện hữu (2025–2026)**

| Giải pháp | Độ trễ đo được / công bố (ghi rõ nguồn) | FPS & Codec | Xuyên NAT tự động | Chi phí & Giấy phép |
| :---- | :---- | :---- | :---- | :---- |
| **Giải pháp đề xuất** | **Mục tiêu 14–22 ms LAN (p50), \<30 ms (p95)** | 30–144 FPS; H.264, HEVC, AV1 | Có (ID/QR \+ IPv6 \+ STUN \+ Relay) | **0 USD, MIT/Apache-2.0** |
| **Moonlight \+ Sunshine** | **8–12 ms (p50), 14–22 ms (p95)** trên LAN Ethernet (bài đo lab Tenvo 05/2026) | Tới 4K HDR 120 FPS; H.264, HEVC, AV1 | Không (cần mở cổng hoặc VPN Tailscale/WireGuard) | 0 USD, GPL-3.0 |
| **Parsec** | Hãng công bố thêm \~7 ms LAN; bài đo Tenvo 05/2026 chỉ chậm hơn Moonlight \~2–6 ms trên LAN và **120–145 ms** ở WAN RTT 50–100 ms | Free: 60 FPS; Warp: 4:4:4 & đa màn | Có (BUD 97% NAT traversal \+ relay Parsec) | Miễn phí cá nhân / 8,33–45 USD/tháng, đóng |
| **AnyDesk** | **16,5 ms** và 59,9 FPS (bài đo độc lập ScienceSoft cho DeskRT) | 60 FPS; DeskRT độc quyền | Có (mạng relay AnyDesk) | Miễn phí cá nhân / 14,90–90,90+ USD/tháng, đóng |
| **RustDesk** | Không có benchmark độc lập chuẩn hóa; phản hồi cộng đồng kém mượt hơn Moonlight/Parsec cho tác vụ động | 30–60 FPS; VP8/VP9/AV1/H.264/HEVC | Có (hbbs/hbbr tự dựng hoặc công cộng) | Miễn phí OSS (AGPL-3.0) / Pro từ 11,88 USD/tháng |
| **TeamViewer** | Không công bố benchmark độ trễ chuẩn tương đương; đánh giá thị trường chậm hơn AnyDesk trên mạng yếu | Thuật toán độc quyền tối ưu văn phòng | Có (chỉ cloud, không cho tự dựng) | Miễn phí cá nhân (dễ bị khóa thương mại) / từ 24,90–50,90+ USD/tháng |

### **Lệnh mẫu: Ghi vết ETW/GPUView trên Windows và Perfetto năng lượng trên Android**

\# 1\. Trên Windows (Admin CMD): ghi vết GPU/DMA vào Merged.etl để mở bằng GPUView  
Log.cmd && timeout /t 30 && Log.cmd

\# 2\. Trên Android (qua adb Wi-Fi, rút cáp sạc, pin 20–85%): ghi Perfetto kèm bộ đếm pin µA/µAh  
adb shell perfetto \-o /data/misc/perfetto-traces/remote\_power.pftrace \-t 60s \\  
  gfx view sched power android.power  
adb pull /data/misc/perfetto-traces/remote\_power.pftrace .

### **Rủi ro đo lường và giảm thiểu**

| Rủi ro đo đạc | Cách giảm thiểu |
| :---- | :---- |
| Lỗi NVIDIA App Overlay khiến tiến trình PresentMon ăn 15–40% CPU và rớt FPS | Tắt hoàn toàn các overlay bên thứ ba (NVIDIA App, Game Bar) trước khi chạy benchmark bằng PresentMon CLI độc lập. |
| Số đo pin Android sai lệch khi cắm cáp USB hoặc pin \>95% / \<20% | Chạy adb qua Wi-Fi không cắm sạc, giữ mức pin trong dải 20–85% và đo đủ ít nhất 3% mức tụt pin theo khuyến nghị học thuật. |

**Danh sách nguồn Agent 8:**

> * Lab benchmark Moonlight vs Parsec (Tenvo, tháng 5/2026): https\://tenvoai.com/blog/moonlight-vs-parsec  
> * So sánh TeamViewer vs AnyDesk (ScienceSoft 16,5 ms, 2026): https\://sobrii.io/blog/teamviewer-vs-anydesk  
> * Microsoft GPUView & ETW: https\://learn.microsoft.com/en-us/windows-hardware/drivers/display/using-gpuview  
> * Android Perfetto Battery Counters & Battery Historian: https\://perfetto.dev/docs/data-sources/battery-counters và https\://developer.android.com/topic/performance/power/battery-historian  
> * Phương pháp đo năng lượng giải mã Android: https\://arxiv.org/html/2110.06529v1

## **Agent 9 – Đóng gói, phân phối, ký mã miễn phí và giao diện tối giản**

### **Phương án đề xuất**

> * **Trình cài đặt Windows:** Chọn **WiX Toolset v5 (.msi)** làm gói cài đặt chính thức cho cả x64 và ARM64 vì hỗ trợ chuẩn ServiceInstall/ServiceControl cho Windows Service quyền LocalSystem và ít bị Windows Defender/AV nhận nhầm hơn tệp .exe của Inno Setup. Bổ sung một bản **Portable .exe** không cần cài đặt (chạy quyền user) cho tình huống hỗ trợ nhanh. Không dùng **MSIX** cho bản chính vì MSIX không hỗ trợ Windows Service chạy dưới tài khoản LocalSystem, bị hạn chế sideload trên Windows 10 và có nguy cơ kẹt cập nhật Store khi Service đang chạy.  
> * **Ký mã Windows & SmartScreen:** Đăng ký **SignPath Foundation** (ký mã miễn phí trên HSM cho dự án mã nguồn mở OSI, tích hợp GitHub Actions) khi dự án đã có cộng đồng và độ nhận diện công khai. Giai đoạn khởi đầu khi chưa đạt ngưỡng nhận diện của SignPath, dùng chứng chỉ **Certum Open Source Code Signing** cho cá nhân phát hành mã nguồn mở phi thương mại (mã kích hoạt từ \~29 USD nếu đã có đầu đọc thẻ, hoặc gói đám mây SimplySign; hiển thị nhà phát hành "Open Source Developer, \<Tên\>"). Từ 2024–2026, ngay cả chứng chỉ EV cũng không còn bỏ qua SmartScreen tức thì mà mọi chứng chỉ đều phải tích lũy uy tín theo lượt cài sạch trên cùng một danh tính ký.  
> * **Phân phối Android:** Phát hành đồng thời trên **GitHub Releases** (cập nhật nhanh, tương thích Obtainium), **F-Droid** (hỗ trợ *Reproducible Builds* không dính thư viện Google Play Services, ký bằng apksigner \--alignment-preserved để người dùng chuyển đổi liền mạch giữa GitHub và F-Droid), và **Google Play** (đảm bảo căn lề bộ nhớ **16 KB page size** bắt buộc cho thư viện .so bằng NDK r28+ và AGP 8.5.1+, trước mốc chặn cập nhật cứng 01/02/2027).

### **Bảng so sánh 3 trình đóng gói Windows**

| Trình đóng gói Windows (Agent 9\) | Trễ (25%) | Điện (20%) | Ảnh (15%) | Ổn định (12%) | Tương thích (10%) | Bảo trì (8%) | Đơn giản (5%) | Pháp lý (5%) | Tổng điểm |
| :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- | :---- |
| Inno Setup (.exe) | 5 | 5 | 5 | 4 | 5 | 5 | 5 | 5 | **4,88** |
| **WiX Toolset v5 (.msi) — Đề xuất cho bản Service** | 5 | 5 | 5 | 5 | 5 | 4 | 3 | 5 | **4,82** |
| MSIX (.msix / Microsoft Store) | 5 | 5 | 5 | 3 | 3 | 4 | 3 | 5 | **4,38** |

Dù Inno Setup nhỉnh hơn 0,06 điểm nhờ độ đơn giản, **WiX v5 (.msi)** được ưu tiên cho bản cài đặt Windows Service nhờ quản trị vòng đời dịch vụ gốc chuẩn xác và giảm thiểu báo động giả của phần mềm diệt virus.

### **Giao diện tối giản và quy trình CI/CD GitHub Actions**

Màn hình chính loại bỏ hoàn toàn thuật ngữ mạng phức tạp, chỉ gồm hai khu vực: **"ID của tôi"** trên Windows (mã 9 chữ số, OTP 6 số có nút đổi mới, mã QR kết nối nhanh) và **"Kết nối tới"** trên Android (ô nhập ID, nút quét mã QR, danh sách máy LAN tự hiện qua mDNS). Trang cài đặt gói gọn 5 nút **Preset một chạm** kèm các mục trực quan: Độ phân giải, FPS (30/60/90/120/144), **Thanh trượt chất lượng 1%–100%** \+ công tắc ABR, Chọn Codec (Tự động / AV1 / HEVC / H.264) và Chế độ xử lý (Phần cứng / Phần mềm).  
\# .github/workflows/release.yml — CI/CD đa kiến trúc Windows x64/ARM64 \+ Android 16 KB  
name: Release Build & Sign  
on:  
  push:  
    tags: \["v\*"\]  
jobs:  
  windows-build:  
    strategy:  
      matrix:  
        target: \[x86\_64-pc-windows-msvc, aarch64-pc-windows-msvc\]  
    runs-on: windows-latest  
    steps:  
      \- uses: actions/checkout@v4  
      \- run: cargo build \--release \--target \${{ matrix.target }} \-p host-service \-p host-ui  
      \- run: dotnet tool install \--global wix && wix build packaging/wix/Package.wxs \-arch $matrix.target=='aarch64-pc-windows-msvc'&&'arm64'||'x64'-odist/remotedesk-${{ matrix.target }}.msi  
      \- uses: signpath/github-action-submit-signing-request@v1  
        with:  
          api-token: \${{ secrets.SIGNPATH\_API\_TOKEN }}  
          artifact-configuration-slug: windows-msi  
  android-build:  
    runs-on: ubuntu-24.04  
    steps:  
      \- uses: actions/checkout@v4  
      \- run: \|  
          cargo ndk \-t arm64-v8a \-t armeabi-v7a \-o android/app/src/main/jniLibs build \--release \-p android-core  
          cd android && ./gradlew assembleFossRelease bundlePlayRelease  
      \# Kiểm tra bắt buộc căn lề 16 KB ELF page size theo yêu cầu Google Play  
      \- run: readelf \-l android/app/src/main/jniLibs/arm64-v8a/libremotedesk.so \| grep \-q "0x4000"  
      \- uses: softprops/action-gh-release@v2  
        with:  
          files: \|  
            android/app/build/outputs/apk/foss/release/\*.apk  
            dist/\*.msi

### **Rủi ro và giảm thiểu**

| Rủi ro đóng gói & phân phối | Cách giảm thiểu |
| :---- | :---- |
| Bị SmartScreen cảnh báo ở các bản đầu tiên | Giữ cố định một danh tính ký mã xuyên suốt các phiên bản, gửi tệp đã ký lên Microsoft Security Intelligence để xét duyệt sạch. |
| F-Droid từ chối vì sai khác byte hoặc lẫn SDK Google | Tách flavor foss sạch 100% (dùng ZXing quét QR), dùng cờ \-ffile\[span\_314\](start\_span)\[span\_314\](end\_span)-prefix-map xóa đường dẫn tuyệt đối và kiểm chứng trên Docker image fdroidserver. |
| Google Play chặn bản cập nhật do lỗi 16 KB page size | Khóa NDK r28+, AGP 8.5.1+, kiểm tra tự động readelf trong CI và dùng sysconf(\_SC\_PAGESIZE) thay vì hằng số 4096 cứng. |

**Danh sách nguồn Agent 9:**

> * Microsoft Code Signing Options (2026): https\://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options  
> * Kinh nghiệm thực tế SignPath Foundation & Certum Open Source (2026): https\://github.com/getdictus/dictus-desktop/issues/38 và https\://my-ssl.com/learn/open-source-code-signing-certificate  
> * WiX Toolset v5 & hạn chế MSIX với Windows Service: https\://www\.apulsoft.ch/blog/windows-msi-plugin-installer-wix5/ và https\://techcommunity.microsoft.com/discussions/msix-discussions/windows-service-msix-automatic-update---in-system-account/3453547  
> * F-Droid Reproducible Builds với Rust \+ NDK (2026): https\://github.com/karmugilen/torrent-player/blob/karmugilen/rust-engine/docs/FDROID\_SUBMISSION.md và https\://github.com/arditore/Seca/blob/main/docs/fdroid.md  
> * Google Play 16 KB Page Size Requirement: https\://developer.android.com/guide/practices/page-sizes

## **Agent 10 – Kiến trúc sư tổng hợp và lộ trình thực thi**

### **Tổng hợp kiến trúc cuối cùng và lý do lựa chọn**

Bốn trụ cột thắng điểm tuyệt đối trong mô hình đánh giá trọng số (ưu tiên độ trễ 25% và điện năng 20%) tạo thành một kiến trúc nhất quán từ phần cứng GPU Windows đến tấm nền màn hình Android: **Rust Core viết mới (4,73/5)** loại bỏ copyleft và tối ưu hiệu năng; **DXGI Desktop Duplication (4,72/5)** đảm bảo zero-copy VRAM và nghỉ khi hình tĩnh; **UDP tùy chỉnh \+ Adaptive RS-FEC (4,77/5)** tránh kẹt đầu hàng TCP/QUIC; và **Noise Protocol (4,95/5)** bảo mật đầu cuối gọn nhẹ không gánh nặng chứng chỉ.

### **Tech Stack cuối cùng, phiên bản và giấy phép**

| Thành phần | Công nghệ / Thư viện đề xuất | Phiên bản (2026) | Giấy phép |
| :---- | :---- | :---- | :---- |
| Lõi dùng chung | **Rust** (tokio, bytes) | Rust Stable 2026 (Edition 2024\) | MIT / Apache-2.0 |
| Bảo mật E2EE & Xác thực | noise-protocol \+ noise-rust-crypto \+ ed25519-dalek \+ spake2 \+ argon2 | noise-protocol 0.2.1, noise-rust-crypto 0.6.2 | Unlicense / MIT / Apache-2.0 |
| Sửa lỗi FEC | reed-solomon-simd (tăng tốc AVX2/NEON) | Bản ổn định mới nhất | MIT / Apache-2.0 |
| Hệ thống & Chụp màn hình Win | windows-rs (DXGI 1.5/1.6, D3D11/12, WGC, WASAPI, WinUser) | Bản ổn định mới nhất | MIT / Apache-2.0 |
| Mã hóa Video Windows | NVENC SDK 13.0, AMD AMF SDK, Intel oneVPL, D3D12 Video, MFT ARM64; SW: **OpenH264** (tải Cisco) / SVT-AV1 | NVIDIA SDK 13.0, OpenH264 2.6.0 | MIT / BSD-2-Clause |
| Âm thanh độ trễ thấp | WASAPI Loopback (IAudioClient3) \+ libopus \+ Android **Oboe/AAudio** | Bản ổn định mới nhất | BSD-3-Clause / Apache-2.0 |
| Giao diện Windows | **Tauri v2** (WebView2) hoặc **WinUI 3** | Tauri v2 / Windows App SDK mới nhất | MIT / Apache-2.0 |
| Ứng dụng Android | **Kotlin \+ Jetpack Compose** \+ MediaCodec → SurfaceView \+ Rust NDK (16 KB); SW: **dav1d** | API 26–36, NDK r28+, AGP 8.5.1+ | Apache-2.0 / BSD-2-Clause |
| Máy chủ Signaling / Relay | Dịch vụ Rust độc lập (signal-relay, Docker) | Tự phát triển | MIT / Apache-2.0 |
| Đóng gói & Ký mã | **WiX Toolset v5** (.msi), SignPath Foundation / Certum OSS, F-Droid \+ GitHub \+ Google Play | WiX v5 | MS-RL / OSS |

### **Cấu trúc thư mục Monorepo**

| Đường dẫn | Chức năng & Phạm vi mã nguồn |
| :---- | :---- |
| /crates/proto | Cấu trúc gói tin nhị phân zero-copy (video, audio, input, control) và hằng số giao thức. |
| /crates/crypto | Bắt tay Noise\_XX/Noise\_IK, xác thực SPAKE2 OTP, chống phát lại 64-bit, quản lý khóa Ed25519. |
| /crates/transport | Socket UDP thống nhất, mDNS, IPv6 Direct, STUN hole punching, Adaptive RS-FEC, GCC ABR, kênh dự phòng TCP/443. |
| /crates/core-win | Chụp DXGI/WGC, shader chuyển màu & HDR tone-map, backend mã hóa HW/OpenH264, SendInput/Touch/Pen/SAS, WASAPI Opus. |
| /apps/win-service | Windows Service Session 0 quyền LocalSystem quản lý phiên, UAC/Secure Desktop và khởi động lại worker. |
| /apps/win-ui | Ứng dụng khay hệ thống và giao diện tối giản ("ID của tôi", QR, hộp thoại xác nhận & cảnh báo chống lừa đảo). |
| /crates/core-android | Thư viện Rust .so (căn lề 16 KB) xử lý mạng, giải mã Noise, ghép khung hình, dav1d và Oboe audio. |
| /apps/android | Dự án Gradle Kotlin/Compose (SurfaceView, MediaCodec low-latency ladder, cử chỉ, PointerCapture, NsdManager, flavor foss/play). |
| /server/signal-relay | Binary Rust hợp nhất cho Signaling (ID/STUN) và Relay (UDP \+ TCP/443). |
| /packaging | Kịch bản WiX v5 (.wxs), metadata F-Droid (fastlane/metadata), workflow CI/CD GitHub Actions. |

### **Định nghĩa giao thức gói tin nhị phân (Wire Protocol v1)**

Mọi gói UDP đều bọc trong khung AEAD Noise 12 byte header \+ 16 byte thẻ xác thực (tổng overhead chỉ 28 byte):

| Loại gói tin | Cấu trúc Header nhị phân (Big-Endian) | Nội dung Payload & Hành vi |
| :---- | :---- | :---- |
| **Khung bọc ngoài (12 byte)** | version\_flags (1B) \| session\_id (4B) \| packet\_type (1B) \| nonce\_counter (6B) | Toàn bộ payload phía sau mã hóa & xác thực bằng nonce\_counter; session\_id hỗ trợ chuyển mạng Wi-Fi↔5G không rớt phiên. |
| **Gói Video (0x01, 16B header)** | frame\_id (4B) \| capture\_ts\_us (4B) \| codec\_flags (1B) \| shard\_idx (1B) \| data\_k (1B) \| parity\_m (1B) \| payload\_len (2B) \| reserved (2B) | Mảnh bitstream (≤1.150 byte tránh phân mảnh MTU) hoặc mảnh parity RS-FEC; capture\_ts\_us phục vụ đồng bộ A/V và thống kê độ trễ. |
| **Gói Âm thanh (0x02, 10B header)** | seq\_num (2B) \| capture\_ts\_us (4B) \| duration\_ms (1B) \| channels\_rate (1B) \| payload\_len (2B) | Khung nén Opus 5–10 ms, gửi kèm RED khi mất gói \>2%. |
| **Gói Nhập liệu (0x03, 6B header)** | input\_seq (2B) \| sub\_type (1B) \| flags (1B) \| payload\_len (2B) | Chuột tương đối/tuyệt đối (tọa độ u16), scan code, chuỗi UTF-8 tiếng Việt, mảng tối đa 10 điểm cảm ứng, bút (áp lực/nghiêng), SAS. |
| **Gói Điều khiển (0x04, 6B header, có ACK)** | ctrl\_seq (2B) \| op\_code (1B) \| reserved (1B) \| payload\_len (2B) | Hello/Caps, DisplayConfigChange, RequestIDR, CursorShape, ClipboardSync, QualitySliderUpdate, PauseVideo, FeedbackStats. |

### **Lộ trình thực thi từ MVP đến bản 1.0 (8 Sprint × 2 tuần \= 16 tuần)**

| Sprint | Mục tiêu triển khai | Tiêu chí nghiệm thu định lượng |
| :---- | :---- | :---- |
| **Sprint 1 (Tuần 1–2)** | Giao thức nhị phân, Noise E2EE & DXGI Capture | Bắt tay Noise\_XX \<5 ms; chụp 1080p60 DXGI zero-copy tải CPU \<3%. |
| **Sprint 2 (Tuần 3–4)** | NVENC/AMF/oneVPL \+ Giải mã Android SurfaceView | Luồng 1080p60 LAN đạt độ trễ glass-to-glass **\<25 ms (p50)** qua bài đo camera/photodiode. |
| **Sprint 3 (Tuần 5–6)** | Nhập liệu đầy đủ, tiếng Việt Unicode, Bút & PointerCapture | Vượt qua 100% kịch bản thử nghiệm: trackpad, chạm trực tiếp, chuột ngoài 360°, bút lực nhấn, gõ tiếng Việt không xung đột UniKey. |
| **Sprint 4 (Tuần 7–8) — MVP** | Xuyên NAT (mDNS, IPv6, STUN, Relay) & Giao diện tối giản | Kết nối bằng ID/OTP và mã QR thành công qua 4G/CGNAT Việt Nam không mở cổng router; độ trễ WAN nội địa **\<75 ms**. |
| **Sprint 5 (Tuần 9–10)** | Windows Service SYSTEM, UAC/Secure Desktop, Đa màn & HDR | Thao tác trọn vẹn màn hình khóa, UAC, Ctrl+Alt+Del; HDR-to-SDR đúng màu; đổi độ phân giải gián đoạn \<150 ms. |
| **Sprint 6 (Tuần 11–12)** | Thanh trượt 1–100%, ABR, Adaptive RS-FEC, Âm thanh Opus | Chịu được mất gói nhân tạo 3% không đứng hình; lệch A/V Opus \<±20 ms; tải encoder \~0% khi màn hình đứng yên. |
| **Sprint 7 (Tuần 13–14)** | Phủ rộng phần cứng (ARM64, OpenH264, lỗi SoC Android) | Chạy ổn định trên ma trận thử nghiệm: Snapdragon X MFT, Intel UHD, máy CPU-only (OpenH264), watchdog tự cứu lỗi MediaTek/Exynos. |
| **Sprint 8 (Tuần 15–16) — v1.0** | WiX v5 .msi, Ký mã, F-Droid/Play 16 KB, Anti-Scam & Audit | CI xuất .msi (x64/ARM64) đã ký và .apk/.aab đạt chuẩn 16 KB; F-Droid reproducible build khớp byte; hoàn tất cảnh báo chống lừa đảo. |

### **Rủi ro lớn nhất và quyết định còn bỏ ngỏ**

Ba rủi ro lớn nhất của dự án là: (1) sự phân mảnh lỗi bộ giải mã phần cứng Android (đặc biệt dòng MediaTek/Unisoc/Amlogic) đòi hỏi duy trì danh sách loại trừ động; (2) cảnh báo SmartScreen trong giai đoạn đầu khi chữ ký mới chưa đủ lượt tải tích lũy uy tín; và (3) nguy cơ kẻ xấu lợi dụng ứng dụng miễn phí cho kịch bản lừa đảo hỗ trợ kỹ thuật. Hai quyết định còn bỏ ngỏ cần chốt trước Sprint 5 gồm: lựa chọn cuối cùng giữa **Tauri v2** (dễ mở rộng đa nền tảng sau này) với **WinUI 3** (thuần Microsoft) cho giao diện Windows, và quyết định có tự bảo trì driver màn hình ảo IddCx ký EV hay tích hợp tùy chọn cài đặt driver IddCx cộng đồng đã ký WHQL để giữ chi phí vận hành bằng 0\.

## **Bao phủ môi trường phần cứng thực tế và 5 Cấu hình gợi ý (Presets)**

### **Ma trận mặc định cho phần cứng Windows (Host)**

| Nhóm phần cứng Windows | Backend mặc định | Codec mặc định | Độ phân giải & FPS khả thi | Lưu ý vận hành |
| :---- | :---- | :---- | :---- | :---- |
| **NVIDIA GTX 10xx – RTX 30xx** | NVENC SDK | HEVC → H.264 | 1080p120 / 1440p120 / 4K60 | Không có HW AV1 encode; độ trễ NVENC \<3–5 ms. |
| **NVIDIA RTX 40xx – RTX 50xx** | NVENC SDK 13.0 | **AV1** (nếu Android có HW) → HEVC | 1440p144 / 4K120 HDR | NVENC thế hệ 8–9 có AV1 HW; RTX 50 thêm 4:2:2 10-bit & tăng 5% chất lượng. |
| **AMD Radeon RX 6000 – RX 9000** | AMF (VCN) | RX 7000/9000: **AV1** → HEVC; RX 6000: HEVC | 1080p120 / 1440p120 / 4K60 | RX 6400, RX 6500 XT và nhân Navi 44 không có HW encoder → tự rơi về OpenH264. |
| **Laptop văn phòng (Intel UHD/Iris Xe/Arc, AMD Vega)** | oneVPL (Intel) / AMF (AMD) | Intel Arc/Meteor Lake+: **AV1** → HEVC; UHD/Vega cũ: **HEVC → H.264** | 1080p60 (\<5 ms trên Intel Gen 12+) | Intel Arc 140T cần oneVPL vì AV1 không hiện trong MFT. |
| **Windows ARM64 (Snapdragon X Elite/Plus)** | **Media Foundation (MFT)** \+ D3D11 | **HEVC** (mặc định ổn định) → H.264 (AV1 tùy chọn) | 1080p60 / 4K60 10-bit | Adreno VPU rất tiết kiệm điện; lưu ý phản ánh lỗi seek ở AV1 đời đầu. |
| **Máy cũ không có HW Encoder** | Phần mềm (**OpenH264** Cisco) | **H.264** (Constrained Baseline) | 720p30 – 1080p30 (1080p60 cần CPU ≥6 nhân) | Bật bỏ qua khung tĩnh tối đa để giảm tải CPU và nhiệt. |

### **Ma trận mặc định cho thiết bị Android (Client)**

| Nhóm thiết bị Android | Khả năng HW Decode | Codec ưu tiên | Lỗi đã biết & Xử lý tự động |
| :---- | :---- | :---- | :---- |
| **Giá rẻ (MediaTek Helio G/P, Unisoc T6xx)** | H.264 1080p60, HEVC 8-bit (không có AV1 HW thật) | **H.264** (bền bỉ và tiết kiệm pin nhất) | Loại trừ unisoc.av1.decoder (thực chất là dav1d SW); kiểm soát trễ decode trên Helio G99. |
| **Tầm trung (Snapdragon 6/7, Dimensity 700–8000)** | H.264, HEVC; **AV1 HW** trên Dimensity 1000+ và SD 7 Gen 4 (2025+) | **HEVC** (trừ nhóm Dimensity dính lỗi Android 15 dùng **H.264**) | Blacklist HEVC HW trên Dimensity 700/900/1080 chạy Android 15; bỏ KEY\_OPERATING\_RATE trên SD 765G. |
| **Cao cấp (Snapdragon 8 Gen 2/3/Elite, Dimensity 9000+, Tensor, Exynos 2200–2500)** | Đầy đủ H.264, HEVC 10-bit, **AV1 10-bit** tới 4K60/1440p120 | **AV1** (WAN) hoặc **HEVC** (LAN) | Watchdog tự hạ H.264 nếu gặp c2.exynos.hevc.decoder Error 0xe trên S21/S24. |
| **Máy tính bảng & Android TV Box** | Tùy SoC (Amlogic S905X3/X4 hỗ trợ AV1 & HEVC) | **HEVC / AV1** | Bỏ cờ low-latency HEVC trên c2.amlogic ngoài Fire TV; tự bật Chạm trực tiếp \+ Bút trên tablet, PointerCapture trên TV Box. |

### **Thích ứng theo 5 môi trường mạng và 5 Cấu hình gợi ý (One-Click Presets)**

Trên **Wi-Fi 5/6/6E cùng phòng**, hệ thống dùng LAN mDNS trực tiếp, tắt FEC, dùng Constrained VBR (15–40 Mbps) và bật WIFI\_MODE\_FULL\_LOW\_LATENCY. Khi **Wi-Fi qua tường** hoặc **kết nối chập chờn mất gói 1–5%**, hệ thống tự bật RS-FEC động 5–20%, rút chu kỳ Intra-Refresh xuống 60 khung và bật gửi lặp RED cho gói nhập liệu. Trên **4G/5G và CGNAT nhà mạng Việt Nam**, ứng dụng ưu tiên **IPv6 Direct** để né CGNAT IPv4 rồi mới tới STUN/Relay Singapore, đồng thời ưu tiên AV1/HEVC để giảm 35–55% dung lượng di động. Trong **mạng công ty chặn UDP**, phiên tự chuyển sang hầm **Relay TCP/TLS cổng 443** với trần 30–60 FPS để giữ ổn định.

| Cấu hình gợi ý (Preset) | Codec ưu tiên | Độ phân giải & FPS | Thanh trượt Chất lượng | Nhập liệu & Tối ưu năng lượng |
| :---- | :---- | :---- | :---- | :---- |
| **1\. Làm việc văn phòng** (Word, Excel, IDE) | HEVC → AV1 → H.264 | 1080p @ **30–60 FPS** (hạ 10 FPS khi tĩnh) | **75%** (ABR bật) | Trackpad tương đối \+ thanh phím nổi \+ tiếng Việt Unicode \+ bỏ khung tĩnh DirtyRect. |
| **2\. Xem video & Trình chiếu** | AV1 → HEVC | 1080p @ **60 FPS** | **70%** (ABR bật) | Chạm trực tiếp \+ đồng bộ Choreographer \+ bật âm thanh **Opus 128 kbps**. |
| **3\. Chơi game nhẹ & Đồ họa** | HEVC → H.264 (LAN) / AV1 (WAN) | 1080p–1440p @ **90–120–144 FPS** | **85%** (CBR, 0-frame buffer) | PointerCapture chuột/phím ngoài hoặc Bút có lực nhấn; xuất thẳng không chờ Vsync. |
| **4\. Hỗ trợ kỹ thuật cho người thân** | H.264 → HEVC | **720p–1080p @ 30 FPS** | **55%** (ABR mạnh, sàn 300 kbps) | Kết nối 1 chạm ID/OTP \+ hộp thoại xác nhận lớn \+ tắt tính năng làm đen màn hình. |
| **5\. Treo máy giám sát dài giờ (24/7)** | HEVC / H.264 (HW) | **720p @ 15–30 FPS** (hạ **5 FPS** khi tĩnh) | **30%** (\~0,5–1,5 Mbps) | Tắt âm thanh, tắt KEY\_LOW\_LATENCY giảm tối đa điện năng; tạm dừng khi tắt màn hình. |

## **Giải đáp 8 câu hỏi kỹ thuật trọng tâm**

### **1\. Bitrate 1080p60 nội dung văn phòng và chênh lệch điện năng giải mã trên Android**

Ở cùng mức độ sắc nét văn phòng 1080p60 (thanh trượt 75%, có bỏ qua khung tĩnh), khi người dùng đang cuộn trang hoặc thao tác liên tục, mô hình tính toán dựa trên tỷ lệ nén thực tế cho thấy **H.264** cần khoảng **6,0–8,0 Mbps**, **HEVC** cần **4,0–5,2 Mbps** (tiết kiệm \~35–44%), và **AV1** cần **3,2–4,2 Mbps** (tiết kiệm \~48–55% so với H.264 và \~20–30% so với HEVC). Trong phiên làm việc thực tế (95% khung hình đứng yên khi gõ chữ/đọc bảng tính), mức băng thông trung bình giảm xuống còn **3–5 Mbps (H.264)**, **1,8–3,0 Mbps (HEVC)** và **1,2–2,2 Mbps (AV1)**.  
Về **chênh lệch điện năng giải mã trên Android**, bộ dữ liệu đo thực nghiệm trên 147 mẫu thiết bị Android khẳng định giải mã phần cứng vượt trội hoàn toàn giải mã phần mềm về hiệu suất năng lượng, trong đó H.264 phần cứng đứng đầu về tiết kiệm điện ở 25–31% trường hợp đo. Hiện chưa có bài đo độc lập chuẩn hóa năm 2026 so sánh trực tiếp chênh lệch mW giữa giải mã *phần cứng* AV1 và *phần cứng* HEVC/H.264 trên cùng một điện thoại cao cấp; dựa trên độ phức tạp khối mạch lớn hơn của AV1, mức chênh lệch phần cứng thường nhỏ (\~5–15% tùy thế hệ SoC, mức độ chắc chắn trung bình). Ngược lại, nếu thiết bị không có phần cứng AV1 và phải chạy **phần mềm dav1d**, phép đo thực tế trên điện thoại phổ thông cho thấy mức hao pin tăng vọt lên 11,04 mAh/phút ở 1080p60 (gần **gấp 2 lần** 720p60 H.264). Vì vậy, chỉ được kích hoạt AV1 khi xác nhận thiết bị có mạch giải mã phần cứng AV1 thật.

### **2\. Có nên dùng YUV 4:4:4 hoặc Screen Content Coding (SCC) để chữ sắc nét hơn không?**

Lấy mẫu phụ YUV 4:2:0 làm nhòe cạnh chữ nhỏ có màu tương phản mạnh (ví dụ chữ đỏ trên nền tối trong IDE/Excel). Chế độ **YUV 4:4:4** giữ đủ độ phân giải màu giúp chữ sắc nét tuyệt đối, còn các công cụ **Screen Content Coding (SCC)** như *Intra Block Copy*, *Palette Mode* và *Transform Skip* (có trong HEVC-SCC và một phần chuẩn AV1) giúp giảm thêm 20–25% bitrate cho giao diện văn bản.  
Tuy nhiên, ba giới hạn phần cứng năm 2026 ngăn cản việc bật 4:4:4 làm mặc định: **(1)** hiện chưa có GPU tiêu dùng nào hỗ trợ mã hóa phần cứng **AV1 4:4:4** (ngay cả RTX 40/50 chỉ hỗ trợ 4:4:4 cho H.264/HEVC và 4:2:2 10-bit cho Blackwell); **(2)** hầu hết bộ giải mã phần cứng MediaCodec trên Android chỉ hỗ trợ đầu ra YUV 4:2:0; và **(3)** YUV 4:4:4 làm tăng băng thông thêm **45% đến gần 100%**. Phương án hợp lý nhất là mặc định **YUV 4:2:0** (ưu tiên **10-bit** khi hai đầu hỗ trợ vì làm mịn chữ và dải màu rõ rệt mà không nhân đôi băng thông), chỉ mở tùy chọn nâng cao **HEVC 4:4:4** trong mạng LAN cho các thiết bị Android đã kiểm chứng giải mã được.

### **3\. Đóng góp độ trễ của từng khâu và khâu dễ cắt giảm nhất**

Trên đường truyền 1080p60 LAN tối ưu với phần cứng hỗ trợ, độ trễ phân rã ước lượng như sau: **Chụp màn hình DXGI zero-copy** 0,5–1,5 ms; **Mã hóa phần cứng Ultra-Low-Latency** 2,0–5,0 ms (so với 30–100 ms nếu mã hóa CPU phần mềm); **Truyền tải mạng** 1,0–3,0 ms (Wi-Fi 6 LAN) hoặc 15–45 ms (Internet/4G nội địa); **Bộ đệm Jitter** 0–3 ms (LAN) hoặc 8–16 ms (WAN); **Giải mã phần cứng Android** 2,0–6,0 ms trên Snapdragon/Tensor cao cấp (nhưng dễ vọt lên 20–28 ms nếu thiếu cờ low-latency hoặc chip MediaTek cũ); và **Hiển thị/Vsync tấm nền** trung bình **8–17 ms ở 60 Hz**, giảm còn **4–8 ms ở 120 Hz**.  
**Khâu dễ cắt giảm nhất** chính là **hàng đợi khung hình ẩn của MediaCodec trên Android** (bật KEY\_LOW\_LATENCY và khóa hãng cắt ngay 50–100 ms tồn đọng), tiếp theo là **bỏ TextureView để xuất thẳng SurfaceView** (cắt 16–33 ms bộ đệm trung gian) và **chuyển lên 120 Hz kết hợp vẽ con trỏ cục bộ**.

### **4\. Xử lý xoay màn hình điện thoại, đổi độ phân giải Windows và cắm thêm màn hình không gián đoạn**

Khi người dùng xoay điện thoại Android, Activity khai báo android:configChanges="orientation\|screenSize\|screenLayout" để không bị hủy/tạo lại; chỉ tính lại tỷ lệ hiển thị trên SurfaceView (hoặc gửi yêu cầu đổi độ phân giải màn hình ảo IddCx nếu người dùng bật tự khớp tỷ lệ). Khi Windows đổi độ phân giải hoặc cắm/rút màn hình, IDXGIOutputDuplication báo DXGI\_ERROR\_ACCESS\_LOST. Tiến trình Windows lập tức khởi tạo lại bộ chụp và bộ mã hóa (hoặc chuyển khe phân giải trong ID3D12VideoEncoderHeap), gửi gói Control::DisplayConfigChange kèm khung **IDR \+ SPS/PPS/OBU Sequence Header mới**. Phía Android kiểm tra FEATURE\_AdaptivePlayback để nạp trực tiếp khung IDR mới mà không cần đóng MediaCodec (gián đoạn \<100 ms), hoặc tự flush() \+ configure() lại trong \~150 ms.

### **5\. Cách đơn giản nhất kết nối qua Internet không mở cổng router với chi phí máy chủ gần bằng 0**

Triển khai kiến trúc **P2P ưu tiên tuyệt đối không cần cấu hình** theo 3 bước: (1) Máy chủ Signaling Rust siêu nhẹ (chạy trên VPS miễn phí Oracle Cloud Always Free hoặc VPS \~4–5 USD/tháng) chỉ cấp ID 9 số và trao đổi địa chỉ ứng viên kết nối; (2) Hai thiết bị tự động thử đồng thời trong 300 ms: **LAN mDNS** → **IPv6 Direct** (xuyên qua CGNAT IPv4 của Viettel/VNPT/FPT mà không tốn băng thông máy chủ) → **STUN \+ UDP Hole Punching** trên cùng một socket UDP bằng máy chủ STUN công cộng miễn phí stun.cloudflare.com:3478; (3) Chỉ \~15% phiên rơi vào Symmetric NAT/chặn UDP đi qua dịch vụ Relay tự dựng (đặt trên gói 10 TB miễn phí/tháng của Oracle Cloud Free Singapore hoặc VPS 20 TB giá \~4,50 USD/tháng), đưa chi phí vận hành thực tế về sát 0 USD mà người dùng không cần động tới router.

### **6\. Truyền âm thanh hệ thống Windows sang Android độ trễ thấp bằng Opus và đồng bộ A/V**

Hệ thống hoàn toàn có thể truyền âm thanh độ trễ thấp (\<25 ms) mà **không cần cài driver âm thanh ảo**. Phía Windows mở **WASAPI Loopback** (IAudioClient3 với cờ AUDCLNT\_STREAMFLAGS\_LOOPBACK trên thiết bị eRender mặc định), dùng InitializeSharedAudioStream đặt kích thước đệm nhỏ 2–5 ms mà không làm mất tiếng loa máy tính, rồi nén bằng **libopus** ở chế độ OPUS\_APPLICATION\_RESTRICTED\_LOWDELAY (khung 5–10 ms, 64–128 kbps stereo). Phía Android giải mã Opus trong Rust và phát qua **Oboe/AAudio** chế độ độ trễ thấp. Để **đồng bộ A/V**, cả khung DXGI và gói PCM đều gắn chung mốc đồng hồ đơn điệu QueryPerformanceCounter của Windows; phía Android luôn hiển thị video ngay lập tức để bảo toàn độ nhạy chuột, còn bộ đệm âm thanh thích ứng (10–25 ms) tự co giãn vi mô tốc độ lấy mẫu (±0,5%) nhằm giữ sai lệch hình–tiếng trong ngưỡng ±15 ms.

### **7\. Có cần AccessibilityService hay quyền đặc biệt trên Android không? Ảnh hưởng tới Google Play?**

**Hoàn toàn KHÔNG cần.** AccessibilityService chỉ cần khi một ứng dụng Android phải đóng vai trò *máy bị điều khiển* để đọc màn hình và tiêm cử chỉ vào các ứng dụng khác. Vì ứng dụng này hoạt động một chiều **Android (Client) → Windows (Host)**, toàn bộ thao tác chạm, bàn phím và chuột diễn ra trong chính cửa sổ ứng dụng. Từ ngày 28/01/2026, Google Play siết rất mạnh chính sách AccessibilityService, yêu cầu tờ khai, video chứng minh và chặn các ứng dụng tự động hóa không phục vụ người khuyết tật. Việc không dùng AccessibilityService, MANAGE\_EXTERNAL\_STORAGE hay SYSTEM\_ALERT\_WINDOW giúp vượt qua kiểm duyệt Google Play thuận lợi chỉ với các quyền mạng chuẩn và CAMERA (tùy chọn quét QR); yêu cầu tuân thủ bắt buộc duy nhất trên Google Play 2026 là hỗ trợ **căn lề bộ nhớ 16 KB** và mức targetSdk quy định.

### **8\. Bẫy pháp lý cần tránh và thiết kế phòng chống lừa đảo hỗ trợ kỹ thuật**

Ba rủi ro pháp lý và uy tín lớn nhất khi phát hành phần mềm điều khiển từ xa miễn phí là: (1) bị kẻ gian lợi dụng cho **lừa đảo hỗ trợ kỹ thuật (Tech Support Scam)** dẫn đến bị hãng diệt virus gắn cờ Riskware hoặc bị yêu cầu chặn hạ tầng relay (thực trạng buộc TeamViewer, AnyDesk, RustDesk đều phải treo cảnh báo lớn); (2) rủi ro rò rỉ dữ liệu người dùng hoặc bị đánh cắp chứng chỉ ký mã; và (3) vi phạm bằng sáng chế codec nếu phân phối sai thư viện H.264/HEVC.  
Để phòng vệ từ gốc kiến trúc, ứng dụng tích hợp năm lớp bảo vệ: (1) hiển thị **cảnh báo chống lừa đảo tiếng Việt** ngay khi mở app lần đầu khuyến cáo không đọc ID/OTP cho người lạ tự xưng ngân hàng/công an/kỹ thuật viên; (2) **tắt mặc định truy cập không giám sát** và bắt buộc bấm **"Cho phép"** kèm dấu vân tay thiết bị trên Windows; (3) **loại bỏ tính năng làm đen màn hình (privacy blackout)** ở chế độ hỗ trợ từ xa và luôn hiện thanh thông báo đang chia sẻ màn hình không thể ẩn; (4) bố trí **nút ngắt khẩn cấp tức thì** (giữ Esc 2 giây hoặc đóng cửa sổ là cắt kết nối ngay); và (5) phát hành theo giấy phép **MIT/Apache-2.0** kèm điều khoản miễn trừ trách nhiệm chuẩn, vận hành máy chủ zero-knowledge không lưu dữ liệu cá nhân và bảo vệ khóa ký mã hoàn toàn trên HSM.

## **Tóm tắt điều hành**

Nghiên cứu này xác lập bản thiết kế kỹ thuật hoàn chỉnh cho một ứng dụng điều khiển máy tính từ xa **Android 8.0+ → Windows 10/11 (x64 & ARM64)** hoàn toàn miễn phí, mã nguồn mở theo giấy phép kép **MIT/Apache-2.0**, không quảng cáo, không giới hạn phiên và không phụ thuộc máy chủ thương mại. Dựa trên khung chấm điểm có trọng số đặt **độ trễ (25%)** và **tiết kiệm năng lượng (20%)** lên hàng đầu, quyết định chiến lược then chốt là **xây dựng mới lõi streaming dùng chung bằng Rust** kết hợp giao diện thuần bản địa siêu nhẹ (**Tauri v2/WinUI 3** trên Windows và **Kotlin/Jetpack Compose \+ NDK** trên Android), thay vì fork RustDesk (ràng buộc AGPL-3.0, độ trễ động chưa tối ưu) hay Sunshine/Moonlight (ràng buộc GPL-3.0, không tự xuyên NAT qua Internet).  
Đường ống hình ảnh được thiết kế theo nguyên lý **zero-copy đầu cuối**: phía Windows chạy mô hình đa tiến trình tách biệt đặc quyền (Windows Service LocalSystem tại Session 0 xử lý Secure Desktop/UAC và tiến trình mạng đặc quyền thấp LOCAL\_SERVICE), chụp màn hình bằng **DXGI Desktop Duplication** trực tiếp từ GPU VRAM vào bộ mã hóa phần cứng bản địa (**NVENC SDK 13.0, AMD AMF, Intel oneVPL** và **Media Foundation** cho Windows ARM64 Snapdragon X), có dự phòng **WGC** cho laptop hai GPU và nhị phân **OpenH264** tải từ Cisco cho máy không có GPU nhằm triệt tiêu hoàn toàn rủi ro bằng sáng chế. Kết hợp loại bỏ B-frame, tắt lookahead, dùng Intra-Refresh, bộ đệm VBV 1 khung hình và bỏ qua khung hình tĩnh (GetFrameDirtyRects), hệ thống giữ tải CPU máy chủ dưới 5% và độ trễ mã hóa phần cứng ở mức 2–5 ms. Phía Android giải mã qua **MediaCodec** chế độ độ trễ thấp (KEY\_LOW\_LATENCY kèm thang khóa riêng cho Qualcomm, Exynos, MediaTek và cơ chế watchdog tự động xử lý lỗi màn hình đen Android 15 trên chip Dimensity), xuất thẳng ra **SurfaceView** và tự hạ nhịp quét màn hình LTPO khi ảnh đứng yên.  
Về mạng và bảo mật, hệ thống dùng **UDP tùy chỉnh mã hóa đầu cuối bằng Noise Protocol (Noise\_XX/Noise\_IK, X25519 \+ ChaCha20-Poly1305)**, kết hợp sửa lỗi **Reed-Solomon thích ứng**, kiểm soát nghẽn dựa trên độ trễ (GCC), truyền âm thanh hệ thống **WASAPI Loopback \+ Opus** đồng bộ lệch dưới ±15 ms, và kênh dự phòng **TCP/443** vượt tường lửa doanh nghiệp. Người dùng phổ thông chỉ cần một chạm (ID 9 số \+ OTP xác thực SPAKE2, quét mã QR, hoặc chọn máy LAN qua mDNS), trong khi chi phí vận hành hạ tầng máy chủ giữ ở mức gần bằng **0 USD** (ước tính 0–4,50 USD/tháng cho 1.000 người dùng hoạt động) nhờ ưu tiên kết nối trực tiếp LAN, **IPv6 Direct** xuyên CGNAT Việt Nam và STUN/UDP hole punching cho khoảng 85% phiên.  
Về trải nghiệm và phát hành, ứng dụng trang bị sẵn **5 cấu hình gợi ý một nút bấm**, **thanh trượt chất lượng phi tuyến 1–100%** tự thích ứng băng thông, gõ tiếng Việt Unicode không xung đột UniKey, chuột ngoài qua PointerCapture, bút cảm ứng có lực nhấn và con trỏ cục bộ giảm cảm giác trễ. Vì chỉ điều khiển một chiều Android → Windows, ứng dụng **không cần quyền AccessibilityService**, tránh hoàn toàn rào cản chính sách 2026 của Google Play; đồng thời đáp ứng sẵn yêu cầu **16 KB page size**, bản build tái lập cho **F-Droid**, trình cài đặt **WiX v5 .msi** ký mã qua **SignPath Foundation/Certum OSS**, và bộ cơ chế chủ động phòng chống lừa đảo hỗ trợ kỹ thuật. Toàn bộ dự án khả thi để triển khai từ **MVP (Tuần 8\)** tới **bản phát hành 1.0 (Tuần 16\)** theo 8 sprint hai tuần với các tiêu chí nghiệm thu định lượng (\<30 ms LAN, \<80 ms WAN).

#### **Nguồn trích dẫn**

1\. scrcpy vs rustdesk \- compare differences and reviews? \- LibHunt, https\://www\.libhunt.com/compare-scrcpy-vs-rustdesk 2\. rustdesk/LICENCE at master \- GitHub, https\://github.com/rustdesk/rustdesk/blob/master/LICENCE 3\. Sunshine \- Self-hosted game stream host for Moonlight. \- GitHub, https\://github.com/SunshineStream/Sunshine 4\. HandBrake Windows on Snapdragon Project \- MulticoreWare Inc, https\://multicorewareinc.com/handbrake-windows-on-snapdragon-project/ 5\. media/java/android/media/MediaFormat.java \- Git at Google, https\://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/media/java/android/media/MediaFormat.java?autodive=0%2F%2F%2F 6\. Parsec vs Sunshine & Moonlight: Ultra-Low-Latency 60+ FPS GPU, https\://nextgen.pk/blog/parsec-vs-sunshine-moonlight-ultra-low-latency-gpu-cloud-gaming-vps-pakistan 7\. noise-protocol — Rust crypto library // Lib.rs, https\://lib.rs/crates/noise-protocol 8\. Video Codec Comparison 2026: AV2 vs VVC, AV1, HEVC, H.264, https\://www\.forasoft.com/learn/video-encoding/articles/codec-comparison-table 9\. H.264 vs HEVC vs VP9 vs AV1: The Developer's Codec Guide, https\://www\.transcodely.com/blog/codec-comparison-h264-hevc-vp9-av1 10\. AV1 Hardware Support in 2026: Encode, Decode, Release Date, https\://www\.forasoft.com/learn/video-encoding/articles/av1-state-2026 11\. OpenH264: Cisco's royalty-free H.264 implementation \- BlogGeek.me, https\://bloggeek.me/webrtcglossary/openh264/ 12\. IDXGIOutputDuplication::GetFrameDirtyRects method (dxgi1\_2.h), https\://learn.microsoft.com/en-us/windows/win32/api/dxgi1\_2/nf-dxgi1\_2-idxgioutputduplication-getframedirtyrects 13\. Yêu cầu băng thông cho Remote Desktop \- Remio Blog, https\://remio.net/vi/blog/remote-desktop-bandwidth-requirements 14\. Vietnam IPv6 Adoption 2026 \- DPTCloud, https\://dptcloud.vn/en/blog/ipv6-dual-stack-bgp-routing-vietnam-2026 15\. Việt Nam đặt mục tiêu tỷ lệ sử dụng IPv6 đạt 72% trong năm nay, https\://vneconomy.vn/viet-nam-dat-muc-tieu-ty-le-su-dung-ipv6-dat-72-trong-nam-nay.htm 16\. Improved UDP hole punching for CGNAT – working PoC ... \- GitHub, https\://github.com/rustdesk/rustdesk/discussions/16488 17\. Moonlight vs Parsec: Latency, Codecs, Setup Cost \- Tenvo, https\://tenvoai.com/blog/moonlight-vs-parsec 18\. Overview \- Parsec support, https\://support.parsec.app/hc/en-us/articles/32361354307348-Overview 19\. Steam Link vs AMD Link vs Parsec vs Rainway vs Sunshine ... \- Reddit, https\://www\.reddit.com/r/cloudygamer/comments/109vbnr/steam\_link\_vs\_amd\_link\_vs\_parsec\_vs\_rainway\_vs/ 20\. Moonlight/Sunshine vs Parsec? : r/cloudygamer \- Reddit, https\://www\.reddit.com/r/cloudygamer/comments/184nxer/moonlightsunshine\_vs\_parsec/ 21\. \[Bug\] H.265/HEVC Playback Fails on MediaTek Dimensity 700 / 900, https\://issuetracker.google.com/issues/437738589 22\. WinUI 3 vs Qt: Comprehensive Guide for Modern Enterprise Apps, https\://softwarelogic.co/en-US/knowledge-base/winui-3-vs-qt-comprehensive-guide-for-modern-enterprise-apps/ 23\. Top 7 desktop app frameworks in 2026 \| MōBrowser Blog \- TeamDev, https\://teamdev.com/mobrowser/blog/top-7-desktop-app-frameworks-in-2026/ 24\. Chrome Remote Desktop Instructions \- Chromium, https\://chromium.googlesource.com/chromium/src/+/refs/tags/147.0.7727.0/remoting/GEMINI.md 25\. RustDesk vs TeamViewer vs AnyDesk: Pricing, Self-Hosting and, https\://meetrix.io/blogs/rustdesk-vs-teamviewer-vs-anydesk/ 26\. Self-host a RustDesk relay server \- SSD Nodes, https\://www\.ssdnodes.com/learn/self-host-rustdesk-relay-server 27\. How is Rustdesk "pro" legal or ethical? Isn't creating proprietary, https\://github.com/rustdesk/rustdesk/discussions/5942 28\. Parsec alternative: Sunshine \+ Moonlight or RustDeck? Any other, https\://www\.reddit.com/r/selfhosted/comments/1w1umip/parsec\_alternative\_sunshine\_moonlight\_or\_rustdeck/ 29\. RustDesk vs AnyDesk (2026): We tried both \- see who wins \- RFP.wiki, https\://www\.rfp.wiki/cloud-computing/desktop-as-a-service/remote-desktop-software/rustdesk/anydesk 30\. Parsec vs Moonlight — Which is Better in 2026? \- Bundl, https\://bundl.run/compare/parsec-vs-moonlight 31\. Best Remote Desktop for Windows in 2026: 7 Apps Tested \- Remio, https\://remio.net/blog/best-remote-desktop-windows 32\. Network guide for Chrome Remote Desktop \- Google Help, https\://support.google.com/chrome/a/answer/16364503?hl=en 33\. Apache Guacamole \- api-evangelist \- GitHub, https\://github.com/api-evangelist/apache-guacamole 34\. remote-cloud-desktop-using-apache-guacamole \- linode/docs, https\://github.com/linode/docs/blob/develop/docs/guides/applications/remote-desktop/remote-cloud-desktop-using-apache-guacamole/index.md 35\. Moonlight vs Parsec vs RDP: GPU Remote Desktop 2026, https\://superrendersfarm.com/article/moonlight-parsec-rdp-remote-desktop-gpu-rendering-2026 36\. selkies-project repositories \- GitHub, https\://github.com/orgs/selkies-project/repositories 37\. Windows Native App Development Is a Mess: A Practical Guide for, https\://dev.to/shuicici/windows-native-app-development-is-a-mess-a-practical-guide-for-2026-43bb 38\. Windows Graphics Capture vs DXGI Desktop Duplication \- OBS Studio, https\://obsproject.com/forum/threads/windows-graphics-capture-vs-dxgi-desktop-duplication.149320/ 39\. D3d11screencapturesrc vs d3d12screencapturesrc \- \#2 by seungha, https\://discourse.gstreamer.org/t/d3d11screencapturesrc-vs-d3d12screencapturesrc/2080/2 40\. nv-codec-headers/include/ffnvcodec/nvEncodeAPI.h at master, https\://github.com/FFmpeg/nv-codec-headers/blob/master/include/ffnvcodec/nvEncodeAPI.h 41\. NVENC Video Encoder API Programming Guide, https\://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/nvenc-video-encoder-api-prog-guide/index.html 42\. Using the Desktop Duplication API with HDR \- Microsoft Learn, https\://learn.microsoft.com/en-us/answers/questions/1457052/using-the-desktop-duplication-api-with-hdr-interpr 43\. How to render to HDR displays on Windows 10 \- pyromuffin, https\://www\.pyromuffin.com/2018/07/how-to-render-to-hdr-displays-on.html 44\. Errors while trying to capture HDR content with DXGI, https\://stackoverflow.com/questions/70629352/errors-while-trying-to-capture-hdr-content-with-dxgi-outputduplication 45\. HDR tone map effect \- Win32 apps \| Microsoft Learn, https\://learn.microsoft.com/da-dk/windows/win32/direct2d/hdr-tone-map-effect 46\. User Account Control Switch to the secure desktop when prompting, https\://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-10/security/threat-protection/security-policy-settings/user-account-control-switch-to-the-secure-desktop-when-prompting-for-elevation 47\. create process in user session from service \- windows \- Stack Overflow, https\://stackoverflow.com/questions/26913172/create-process-in-user-session-from-service 48\. Indirect Display Driver Model Overview \- Windows \- Microsoft Learn, https\://learn.microsoft.com/en-us/windows-hardware/drivers/display/indirect-display-driver-model-overview 49\. Intel® oneAPI Video Processing Library Installation Guide, https\://www\.intel.com/content/www/us/en/developer/articles/guide/onevpl-installation-guide.html 50\. D3D12 Video Encoding \- Windows drivers \- Microsoft Learn, https\://learn.microsoft.com/en-us/windows-hardware/drivers/display/video-encoding-d3d12 51\. Agility SDK 1.716.0-preview: New D3D12 Video Encode Features, https\://devblogs.microsoft.com/directx/agility-sdk-1-716-0-new-d3d12-video-encode-features/ 52\. D3D12 Video Encoding \| DirectX-Specs \- Microsoft Open Source, https\://microsoft.github.io/DirectX-Specs/d3d/D3D12VideoEncoding.html 53\. Ra mắt Snapdragon X Elite: Đỉnh cao chip ARM laptop, out trình đối, https\://www\.thegioididong.com/tin-tuc/ra-mat-snapdragon-x-elite-dinh-cao-chip-arm-laptop-out-trinh-doi-thu-1553035 54\. Ciscobinary.openh264.org is unreachable in some countries (RU, https\://discussion.fedoraproject.org/t/ciscobinary-openh264-org-is-unreachable-in-some-countries-ru-ua-ir/161434/37 55\. New AI SDKs and Tools Released for NVIDIA Blackwell GeForce, https\://developer.nvidia.com/blog/new-ai-sdks-and-tools-released-for-nvidia-blackwell-geforce-rtx-50-series-gpus/ 56\. GeForce RTX 50 Series GPUs Double Creative Performance, https\://blogs.nvidia.com/blog/generative-ai-studio-ces-geforce-rtx-50-series/ 57\. NVIDIA Video Codec SDK 13.0 Powered by NVIDIA Blackwell, https\://developer.nvidia.com/blog/nvidia-video-codec-sdk-13-0-powered-by-nvidia-blackwell/ 58\. AMD RDNA 4: AV1 coding and limitations for entry-level GPUs, https\://www\.igorslab.de/en/amd-rdna-4-av1-coding-and-restrictions-for-beginner-gpus/ 59\. Video Core Next \- Wikipedia, https\://en.wikipedia.org/wiki/Video\_Core\_Next 60\. Comparison between NVENC (NVIDIA) and AMF (AMD) encoder for, https\://www\.reddit.com/r/obs/comments/1li4ey3/comparison\_between\_nvenc\_nvidia\_and\_amf\_amd/ 61\. AV1 encoding not available ? Poor video encoding \- Intel Community, https\://community.intel.com/t/av1-encoding-not-available-poor-video-encoding/143027 62\. AV1 vs H.264 vs VP9 vs VP8 \| Video Codec Guide 2026, https\://www\.digitalsamba.com/blog/video-codec-guide 63\. Third-party licenses \- Remtivo, https\://app.remtivo.com/licenses 64\. Access Advance Welcomes Wave of New Licensees to the HEVC, https\://mediaconnect.com/access-advance-welcomes-wave-of-new-licensees-to-the-hevc-advance-patent-pool/ 65\. Access Advance Announces HEVC Advance and VVC Advance, https\://accessadvance.com/2025/07/21/access-advance-announces-hevc-advance-and-vvc-advance-pricing-through-2030/ 66\. Video Codecs Explained: H.264, H.265, AV1 & VP9 \[2026 Update\], https\://antmedia.io/video-codecs-streaming-guide/ 67\. AV1 \- Wikipedia, https\://en.wikipedia.org/wiki/AV1 68\. x264 vs x265 vs AV1 & NVENC: 2026 Quality-Speed Test \- Fora Soft, https\://www\.forasoft.com/learn/video-quality/articles-vqm/encoder-comparison-x264-x265-svt-av1 69\. AV1 Decoding: Device Compatibility and Upgrade Guide, https\://androidtvbox.eu/av1-decoding-streaming-devices-guide/ 70\. NVIDIA NVENC Obs Guide \| GeForce News, https\://www\.nvidia.com/en-us/geforce/guides/broadcasting-guide/ 71\. CPU encoding (x264, AOM AV1 and SVT AV1) put 100 % load on, https\://obsproject.com/forum/threads/cpu-encoding-x264-aom-av1-and-svt-av1-put-100-load-on-gpu.183974/ 72\. AV1 vs H.264 vs HEVC: Which Is Better? 2026 Test \- Fora Soft, https\://www\.forasoft.com/learn/video-quality/articles-vqm/codec-comparison-real-content 73\. MediaCodec \| API reference \- Android Developers, https\://developer.android.com/reference/android/media/MediaCodec 74\. Which has the most low latency between NDK or Java MediaCodec, https\://stackoverflow.com/questions/38860292/which-has-the-most-low-latency-between-ndk-or-java-mediacodec-api 75\. MediaCodec Extensions \- Android on Snapdragon – Video Features, https\://docs.qualcomm.com/bundle/publicresource/topics/80-56386-10/mediacodec\_extensions.html?cmpid=pr-hsoDreUVz3\&utm\_medium=pr\&utm\_source=Articles\&utm\_campaign=QDN-Content-Syndication-fy23 76\. Media \- NDK \- Android Developers, https\://developer.android.com/ndk/reference/group/media 77\. Low-latency decoding in MediaCodec \| Android OS core topics, https\://source.android.com/docs/core/media/low-latency-media 78\. How to reduce Android hw video decoding latency? 30ms for H264, https\://stackoverflow.com/questions/53605411/how-to-reduce-android-hw-video-decoding-latency-30ms-for-h264-720p 79\. Frame rate \| Android media \- Android Developers, https\://developer.android.com/media/optimize/performance/frame-rate 80\. core/java/android/view/Surface.java \- platform/frameworks/base, https\://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/view/Surface.java?autodive=0%2F 81\. android.net.wifi.WifiManager \- Android Developers, https\://developer.android.com/sdk/api\_diff/29/changes/android.net.wifi.WifiManager 82\. android.net.wifi.WifiManager \- HCLSoftware Open Source, http\://opensource.hcltechsw.com/volt-mx-native-function-docs/Android/android.net.wifi-Android-10.0/\#\!/api/android.net.wifi.WifiManager 83\. QUIC vs TCP/UDP: Rethinking Transport for WebRTC Media, https\://www\.naevatec.com/blog/quic-vs-tcp-udp-webrtc-media-transport 84\. Forward? Error? Correction? \- Media over QUIC, https\://moq.dev/blog/forward-error-correction 85\. WebRTC ICE, STUN and TURN Explained: How Calls Cross NAT, https\://www\.forasoft.com/learn/video-streaming/articles-streaming/nat-stun-turn-ice-webrtc 86\. NAT Traversal and UDP Hole Punching: STUN, TURN, and ICE in, https\://vpnrice.com/news/nat-traversal-udp-hole-punching-stun-turn-ice-vpn 87\. Use network service discovery \| Connectivity \- Android Developers, https\://developer.android.com/develop/connectivity/wifi/use-nsd 88\. When Packets Can't Wait: Comparing Protocols for Delay-Sensitive, https\://www\.rationali.st/delay-sensitive-protocols/ 89\. RTMP vs. SRT vs. WebRTC: Choosing the Right Live Streaming, https\://www\.mux.com/articles/rtmp-vs-srt-vs-webrtc-live-streaming-ingest-protocols 90\. Let's go door with KCP \- Virus Bulletin, https\://virusbulletin.com/uploads/pdf/conference/vb2023/papers/Lets-go-door-with-KCP.pdf 91\. GitHub \- p9c/kcp: kcp reliable udp low latency network transport, https\://github.com/p9c/kcp 92\. Comparison of QUIC-based and WebRTC Protocols \- arXiv, https\://arxiv.org/html/2505.22132v1 93\. Choosing the right transport protocol: TCP vs. UDP vs. QUIC, https\://www\.haproxy.com/blog/choosing-the-right-transport-protocol-tcp-vs-udp-vs-quic 94\. QUIC vs. TCP—Development and Monitoring Guide \- LogicMonitor, https\://www\.logicmonitor.com/deep-dive/http3-vs-http2/quic-vs-tcp 95\. Android NSD can't resolve the mDNS service in Windows, https\://stackoverflow.com/questions/75529457/android-nsd-cant-resolve-the-mdns-service-in-windows 96\. NAT Traversal: How It Works \- DEV Community, https\://dev.to/alakkadshaw/nat-traversal-how-it-works-4dnc 97\. TURN server in WebRTC: when you need it and what it costs, https\://bloggeek.me/webrtcglossary/turn/ 98\. Quyết định 3369/QĐ-BKHCN 2025 chuyển đổi IPv6 only cho Việt, https\://thuvienphapluat.vn/van-ban/Cong-nghe-thong-tin/Quyet-dinh-3369-QD-BKHCN-2025-chuyen-doi-IPv6-only-cho-Viet-Nam-giai-doan-2026-2030-678998.aspx 99\. INPUT structure (winuser.h) \- Win32 apps \- Microsoft Learn, https\://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-input 100\. KEYBDINPUT structure (winuser.h) \- Win32 apps \- Microsoft Learn, https\://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-keybdinput 101\. Using SendInput to send unicode characters beyond U+FFFF, https\://stackoverflow.com/questions/22291282/using-sendinput-to-send-unicode-characters-beyond-uffff 102\. InjectSyntheticPointerInput function (winuser.h) \- Win32 apps, https\://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-injectsyntheticpointerinput 103\. POINTER\_PEN\_INFO (winuser.h) \- Win32 apps \| Microsoft Learn, https\://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-pointer\_pen\_info 104\. InjectTouchInput function (winuser.h) \- Win32 apps \| Microsoft Learn, https\://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-injecttouchinput 105\. SendSAS function (sas.h) \- Win32 apps \| Microsoft Learn, https\://learn.microsoft.com/en-us/windows/win32/api/sas/nf-sas-sendsas 106\. Track touch and pointer movements \| Views \- Android Developers, https\://developer.android.com/develop/ui/views/touch-and-input/gestures/movement 107\. Các tính năng nâng cao của bút cảm ứng \| Jetpack Compose, https\://developer.android.com/develop/ui/compose/touch-input/stylus-input/advanced-stylus-features?hl=vi 108\. UniKey \- Bàn phím tiếng Việt \| UniKey, https\://www\.unikey.org/ 109\. UniKey Vietnamese Keyboard, https\://www\.unikey.org/en/ 110\. First-person shooter on Android – how to process mouse events?, https\://stackoverflow.com/questions/22257649/first-person-shooter-on-android-how-to-process-mouse-events 111\. View.RequestPointerCapture Method (Android.Views), https\://learn.microsoft.com/en-us/dotnet/api/android.views.view.requestpointercapture?view=net-android-37.0 112\. What can I do about my pen input not working at the Ctrl+Alt+Del, https\://support.wacom.com/hc/en-us/articles/28273184256535-What-can-I-do-about-my-pen-input-not-working-at-the-Ctrl-Alt-Del-screen-and-in-System-prompts-in-the-remote-session 113\. noise\_rust\_crypto \- Rust \- Docs.rs, https\://docs.rs/noise-rust-crypto 114\. TLS 1.3 Versus Noise Protocol \- wolfSSL, https\://www\.wolfssl.com/tls-1-3-versus-noise-protocol/ 115\. Remote Desktop Protocol and PQC TLS 1.3 Hybrid, https\://techcommunity.microsoft.com/blog/post-quantum-crypto-tech-blog/remote-desktop-protocol-and-pqc-tls-1-3-hybrid/4562178 116\. HelpBuoy: remote support that scammers won't use \| TQdev, https\://www\.tqdev.com/2026-helpbuoy-secure-self-hosted-remote-support/ 117\. Performance Comparison of EDHOC and DTLS 1.3 in Internet ... \- HAL, https\://hal.science/hal-04382397v1/document 118\. About GPUView \- Windows drivers \| Microsoft Learn, https\://learn.microsoft.com/en-us/windows-hardware/drivers/display/using-gpuview 119\. GPUView Tool Installation and Usage Guide \| PDF \- Scribd, https\://www\.scribd.com/document/476054947/GPUView 120\. Analyze power use with Battery Historian \| App quality, https\://developer.android.com/topic/performance/power/battery-historian 121\. Power data sources \- Perfetto Tracing Docs, https\://perfetto.dev/docs/data-sources/battery-counters 122\. Intel QSV Transcoding Guide: FFmpeg & VAAPI \- Vajracast, https\://vajracast.com/blog/hardware-transcoding-intel-qsv/ 123\. AMD GPU Hardware Encoding Guide: Compatibility Analysis for, https\://intl.cloud.baidu.com/article/9102770 124\. r/AV1 on Reddit: dav1d battery drain test on a budget phone, Part 4, https\://www\.reddit.com/r/AV1/comments/1chpz2r/dav1d\_battery\_drain\_test\_on\_a\_budget\_phone\_part\_4/ 125\. How To Add Windows Services with WiX Installer, https\://www\.advancedinstaller.com/versus/wix-toolset/wix-installer-add-windows-services.html 126\. Windows .msi Plugin Installer using WiX v5 \- apulSoft Blog, https\://www\.apulsoft.ch/blog/windows-msi-plugin-installer-wix5/ 127\. Proactive Measures To Prevent Inno Setup Installer From Getting, https\://forum.juce.com/t/proactive-measures-to-prevent-inno-setup-installer-from-getting-flagged-by-windows-defender/65217 128\. Windows Service Msix Automatic Update \- in SYSTEM ACCOUNT, https\://techcommunity.microsoft.com/discussions/msix-discussions/windows-service-msix-automatic-update---in-system-account/3453547 129\. MSIX on Windows 10 and Windows 11 \- Microsoft Learn, https\://learn.microsoft.com/en-us/windows/apps/package-and-deploy/msix-windows10-windows11 130\. App Update Blocked by Running Windows Service \- Microsoft Q\&A, https\://learn.microsoft.com/en-us/answers/questions/3873689/wpf-published-to-microsoft-store-app-update-blocke 131\. Windows code signing via SignPath Foundation (free OSS program), https\://github.com/getdictus/dictus-desktop/issues/38 132\. The free Code Signing & Software Integrity solution for Open Source, https\://signpath.io/solutions/open-source-community 133\. SignPath Foundation, https\://signpath.org/ 134\. Open Source Code Signing Certificates in 2026 \- My-SSL, https\://my-ssl.com/learn/open-source-code-signing-certificate 135\. Open Source Code Signing \- code \- Certum Store, https\://certum.store/open-source-code-signing-code.html 136\. Code signing options for Windows app developers \- Microsoft Learn, https\://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options 137\. Publish the app's .apk from Google Play and F-Droid to GitHub, https\://github.com/super-productivity/super-productivity/issues/3596 138\. PriveeApp/docs/FDROID.md at main \- MaxDac \- GitHub, https\://github.com/MaxDac/PriveeApp/blob/main/docs/FDROID.md 139\. FDROID\_SUBMISSION.md \- karmugilen/torrent-player \- GitHub, https\://github.com/karmugilen/torrent-player/blob/karmugilen/rust-engine/docs/FDROID\_SUBMISSION.md 140\. Seca/docs/fdroid.md at main \- GitHub, https\://github.com/arditore/Seca/blob/main/docs/fdroid.md 141\. Support 16 KB page sizes \| Compatibility \- Android Developers, https\://developer.android.com/guide/practices/page-sizes 142\. \[Android\] Native library does not support 16KB page size \- GitHub, https\://github.com/ThinkSys/mediapipe-reactnative/issues/21 143\. Google Play's 16 KB page size requirement: A guide \- Median.co, https\://median.co/blog/how-to-prepare-android-apps-google-play-16-kb-page-size-requirement 144\. Can someone on a Snapdragon X2 test the AV1 encoder? \- Reddit, https\://www\.reddit.com/r/snapdragon/comments/1u2r5oe/can\_someone\_on\_a\_snapdragon\_x2\_test\_the\_av1/ 145\. Video Streaming with the AV1 Video Codec in Mobile Devices, https\://engineering.fb.com/wp-content/uploads/2025/09/Meta-AV1-White-Paper-FINAL.pdf 146\. Power Consumption of Video-Decoders on Various Android Devices, https\://arxiv.org/html/2110.06529v1 147\. MSU Mobile Video Codecs Benchmark, https\://videoprocessing.ai/benchmarks/mobile-video-codec-benchmark.html 148\. The Pros and Cons of Software Decoding for Publishers and Viewers, https\://www\.streamingmediaglobal.com/Articles/News/Featured-News/The-Pros-and-Cons-of-Software-Decoding-for-Publishers-and-Viewers-169772.aspx 149\. Tunneling High-Resolution Color Content through 4:2:0 HEVC and, https\://www\.microsoft.com/en-us/research/wp-content/uploads/2016/02/DCC1320Wu\_et\_al20Hi-Res20Color.pdf 150\. Overview of Screen Content Video Coding \- ResearchGate, https\://www\.researchgate.net/publication/311443280\_Overview\_of\_Screen\_Content\_Video\_Coding\_Technologies\_Standards\_and\_Beyond 151\. Overview of Screen Content Coding in Recently Developed Video, https\://arxiv.org/pdf/2011.14068 152\. How to enable YUV 444 with AV1 encode? : r/MoonlightStreaming, https\://www\.reddit.com/r/MoonlightStreaming/comments/1tslkct/how\_to\_enable\_yuv\_444\_with\_av1\_encode/ 153\. Can't stream with chroma 4:4:4 but GPU is capable of encoding, https\://github.com/Nonary/Vibepollo/issues/729 154\. Decoding support of YUV 4:4:4 \- NVIDIA Developer Forums, https\://forums.developer.nvidia.com/t/decoding-support-of-yuv-44/47857 155\. Graphics encoding over the Remote Desktop Protocol, https\://docs.azure.cn/en-us/virtual-desktop/graphics-encoding 156\. D3D12 AV1 Video Encoding \- Windows drivers \- Microsoft Learn, https\://learn.microsoft.com/en-us/windows-hardware/drivers/display/video-encoding-d3d12-av1 157\. Low Latency Audio \- Windows drivers \| Microsoft Learn, https\://learn.microsoft.com/en-us/windows-hardware/drivers/audio/low-latency-audio 158\. WASAPI: A deep dive into Windows desktop capture APIs \- Recall.ai, https\://www\.recall.ai/blog/windows-wasapi 159\. Loopback Recording \- Win32 apps \| Microsoft Learn, https\://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording 160\. WASAPI Loopback Translation — Driverless, No VB-CABLE, https\://voxislive.com/wasapi-loopback-translation 161\. GitHub \- emrecankarli/AudioSender: Ultra-low latency audio, https\://github.com/emrecankarli/AudioSender 162\. High-performance audio \- NDK \- Android Developers, https\://developer.android.com/ndk/guides/audio 163\. Impact of Accessibility Permission in Android Apps \- BrowserStack, https\://www\.browserstack.com/app-accessibility-testing/android/accessibility-permission-in-android 164\. Permissions and APIs that Access Sensitive Information \- Google Help, https\://support.google.com/googleplay/android-developer/answer/16558241?hl=en 165\. Google Play Accessibility Services Policy Update \- My App Monitor, https\://myappmonitor.com/blog/google-play-accessibility-services-policy-update 166\. Use of the AccessibilityService API \- Play Console Help \- Google Help, https\://support.google.com/googleplay/android-developer/answer/10964491?hl=en 167\. Why I removed AccessibilityService from my Android app (and finally, https\://dev.to/lui61140/why-i-removed-accessibilityservice-from-my-android-app-and-finally-got-into-play-store-4i52 168\. Understanding Google Play's 16 KB Page Size Requirement for, https\://dev.to/dainyjose/understanding-google-plays-16-kb-page-size-requirement-for-android-apps-phd 169\. RustDesk: Open-Source Remote Desktop with Self-Hosted Server, https\://rustdesk.com/ 170\. Remote Desktop Software Team & Company Information \- RustDesk, https\://rustdesk.com/team 171\. TeamViewer vs AnyDesk 2026: Full Comparison \- sobrii, https\://sobrii.io/blog/teamviewer-vs-anydesk 172\. Chrome Remote Desktop: All You Need to Know about google, https\://cloudzy.com/blog/all-you-need-to-know-about-chrome-remote-desktop-and-how-to-use-it/