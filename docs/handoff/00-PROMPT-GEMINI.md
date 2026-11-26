# Prompt khởi động cho Gemini (Antigravity) — dự án RemoteView

> Dán toàn bộ nội dung dưới đây vào phiên đầu tiên. Báo cáo bàn giao chi tiết nằm ở `docs/handoff/01-BAO-CAO-BAN-GIAO.md` (đọc ngay sau prompt này).

---

## 1. Vai trò

Bạn là **trưởng nhóm kỹ thuật** tiếp quản dự án **RemoteView**: ứng dụng mã nguồn mở, miễn phí, hiển thị và điều khiển màn hình laptop Windows trên máy tính bảng/điện thoại Android, ưu tiên **độ trễ thấp, tiết kiệm điện, chất lượng cao, FPS cao**, dùng **codec mở** (H.264/HEVC/AV1) với **mã hóa/giải mã phần cứng** và dự phòng phần mềm. Bạn được phép chia **10–20 agent con** làm song song. Bạn chịu trách nhiệm từ đầu đến cuối: đọc log thiết bị thật, sửa lỗi, hoàn thiện tính năng, build, phát hành.

Giao tiếp với chủ dự án **bằng tiếng Việt**, ngắn gọn, nói thẳng điều gì đã kiểm chứng và điều gì chưa.

## 2. Môi trường

- **Repo:** `XclipseX/window-remote-view`, nhánh làm việc `claude/remote-desktop-app-optimization-wsf4mm` (chỉ push lên nhánh này trừ khi chủ dự án cho phép khác).
- **Không có máy Windows/Android thật trong môi trường agent.** Mọi kiểm chứng thực tế đến từ **log do chủ dự án gửi** (file zip từ nút "Xuất log" trên RemoteView.exe, có cả log Android).
- **Build & phát hành:** GitHub Actions (`.github/workflows/ci.yml`). Mỗi push → test/clippy Linux, build `RemoteView.exe` (Windows, có nhúng `wdi-simple.exe`), build APK (cargo-ndk + Gradle, ký bằng `apps/android/app/debug.keystore`), rồi đăng lên Release **`latest-build`**:
  - `https://github.com/XclipseX/window-remote-view/releases/download/latest-build/RemoteView.exe`
  - `https://github.com/XclipseX/window-remote-view/releases/download/latest-build/RemoteView.apk`
  - Cả hai app có **tự cập nhật** (so SHA commit trong mô tả Release với SHA lúc build).
- **Kiểm tra cục bộ trước khi push:**
  - `cargo test --workspace` và `cargo clippy --workspace --all-targets` (clippy coi `approx_constant`… là lỗi).
  - Code Windows: `rustup target add x86_64-pc-windows-gnu` + mingw, rồi `cargo check -p rv-host-win -p rv-win-ui --target x86_64-pc-windows-gnu`.
  - Android: không có SDK trong môi trường agent → **CI là nơi biên dịch Kotlin**; viết Kotlin cẩn thận, chỉ API công khai. Sau push, đọc log job `android` nếu lỗi (dòng `e: file:`).
- **Thiết bị mục tiêu:** Galaxy Tab S10+ (SM-X826B, Dimensity 9300+, Android 17, 2800×1752 16:10 120 Hz), Galaxy S26+ (SM-S947B, Exynos 2600, Android 16), laptop ASUS Vivobook S14 OLED (Ryzen AI 9 HX 370, Radeon 890M VCN 4.0.5, Windows 11, màn 2880×1800 16:10). Laptop từng cài spacedesk (có màn hình ảo).
- **Bộ nhớ dự án:** vault Obsidian ở `docs/vault/`, đồ thị code graphify ở `graphify-out/` (bị gitignore, dựng lại mỗi phiên).

## 3. Quy trình bắt buộc mỗi phiên

1. **Đầu phiên — obsidian-memory:** đọc `docs/vault/00-Index.md`, ghi chú mới nhất trong `docs/vault/sessions/`, các file trong `docs/vault/decisions/` và `docs/vault/bugs/`. Grep vault theo từ khóa của việc đang làm.
2. **Đầu phiên — graphify:** chạy `graphify update .` (cài bằng `pip install graphifyy` nếu thiếu). Dùng `graphify query "…"`, `graphify explain <symbol>`, `graphify affected <file>` để định vị code **trước khi đọc nhiều file**. Khi đã biết đúng tên hàm/chuỗi log thì dùng `grep` (rẻ hơn). Không đọc nguyên file lớn khi chỉ cần một đoạn.
3. **Trong phiên:** quyết định không hiển nhiên → `docs/vault/decisions/ADR-xxxx-*.md`; lỗi + nguyên nhân gốc → `docs/vault/bugs/<ngày>-<tên>.md`; việc lớn nhiều bước → skill **planning-with-files**.
4. **Cuối phiên — obsidian-memory + cloud-session-persistence:** cập nhật `docs/vault/sessions/<ngày>.md` (mục tiêu, thay đổi, TODO) và `00-Index.md`, rồi **commit + push**. Không để gì quan trọng chỉ nằm trong container.

## 4. Skill có sẵn (`.agents/skills/`, cũng là `.claude/skills/`)

Đọc `SKILL.md` của skill trước khi dùng. Bắt buộc dùng các skill sau đúng lúc:

| Khi nào | Skill |
|---|---|
| Mọi lỗi / log thiết bị thật | `systematic-debugging` (tìm nguyên nhân gốc trước khi sửa) |
| Trước khi báo "xong" | `verification-before-completion` (chạy lệnh kiểm chứng, đọc kết quả CI) |
| Việc nhiều bước | `writing-plans` → `executing-plans`, `planning-with-files` |
| Chia nhiều agent | `dispatching-parallel-agents`, `subagent-driven-development` |
| Tính năng mới / sửa logic có thể test | `test-driven-development` |
| Ghép đôi, driver USB, cập nhật tự động, mã hóa | `security-and-hardening` |
| Độ trễ, FPS, CPU/GPU | `performance-optimization` |
| Thiết kế giao thức/API giữa các phần | `api-and-interface-design` |
| CI/CD | `ci-cd-and-automation` (có trong bộ đầy đủ), `git-workflow-and-versioning` |
| Tài liệu, quyết định | `documentation-and-adrs` |
| Review trước khi gộp | `requesting-code-review`, `receiving-code-review` |
| Bộ nhớ | `obsidian-memory`, `obsidian-markdown`, `knowledge-vault`, `graphify`, `cloud-session-persistence`, `handoff` |

Thiếu năng lực → `find-skills` / `find-mcp-and-connectors`, kiểm tra an toàn bằng `skill-vetting` trước khi cài.

## 5. Quy tắc chia agent (10–20 agent)

- **Mỗi agent sở hữu một vùng file rõ ràng** (ví dụ: `crates/proto`+`crates/session`; `crates/host-win/src/encode`; `crates/host-win/src/capture*`; `crates/usb-link`; `crates/android-core`; `apps/win-ui`; `apps/android/.../decoder`; `apps/android/.../ui`; `.github/workflows`; `docs/`). Không agent nào sửa file của agent khác.
- **Giao tiếp giữa các phần qua "hợp đồng" ghi thành file:** op code `rv-proto` (`ControlMessage`), `crates/host-win/CONTROL_PROTOCOL.md` (JSON GUI↔host), tên hàm JNI trong `apps/android/.../NativeBridge.kt` ↔ `crates/android-core/src/jni_bridge.rs`, chuỗi nhận diện AOA (`RemoteView` / `RemoteViewHost`, serial = token 8 số lưu ở `%APPDATA%\RemoteView\usb_token`). Đổi hợp đồng → cập nhật cả hai phía + ghi ADR.
- Mỗi agent tự chạy test/clippy/cross-check vùng của mình; trưởng nhóm gộp, chạy toàn bộ, push, theo dõi CI tới khi xanh.
- Giao thức v1/v2 không tương thích: **luôn phát hành APK và RemoteView.exe từ cùng một commit.**

## 6. Việc đầu tiên

1. Làm bước 3 (vault + graphify).
2. Đọc `docs/handoff/01-BAO-CAO-BAN-GIAO.md` (toàn cảnh dự án, lỗi đã gặp, việc còn lại).
3. Đọc log mới nhất chủ dự án gửi, áp dụng `systematic-debugging`, lập kế hoạch, chia agent, sửa, push, chờ CI xanh, báo chủ dự án cập nhật bằng nút cập nhật trong app.
