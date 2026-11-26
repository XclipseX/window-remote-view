---
type: bug
date: 2026-11-26
---
# AMD MF encoder: ProcessInput MF_E_NOTACCEPTING (0xC00D36B5) on every frame
- Thiết bị: Vivobook S14 (Radeon 890M), AMDh265Encoder / AMDh264Encoder (async MFT).
- Triệu chứng: Tab màn hình đen; host log "waiting for first video frame", 159x NOTACCEPTING; StreamConfig gửi lại mỗi ~0.5 s.
- Nguyên nhân: (1) async MFT phát nhiều METransformNeedInput hơn số input thực nhận → NOTACCEPTING; (2) fail_streak bị reset mỗi lần dựng lại encoder nên không bao giờ fallback.
- Sửa: retry sau khi drain event, bỏ frame nếu vẫn bận; không reset fail_streak khi tạo encoder.
