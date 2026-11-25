package com.remoteview.client.input

// UNBUILT: chưa biên dịch.

enum class MouseButtonId { LEFT, RIGHT, MIDDLE, X1, X2 }

enum class TouchPhase { DOWN, MOVE, UP, CANCEL }

/** Sự kiện nhập liệu trung lập, được lõi Rust đóng gói gửi sang host Windows. */
sealed interface RemoteInput {
    /** Vị trí tuyệt đối chuẩn hóa 0..1 trên khung hình host. */
    data class MouseMoveAbs(val nx: Float, val ny: Float) : RemoteInput
    data class MouseMoveRel(val dx: Int, val dy: Int) : RemoteInput
    data class MouseButton(val button: MouseButtonId, val down: Boolean) : RemoteInput

    /** Đơn vị WHEEL_DELTA (120 = một nấc). vertical > 0 = cuộn lên. */
    data class MouseWheel(val horizontal: Int, val vertical: Int) : RemoteInput

    /** Scancode Set 1 (PS/2) như SendInput KEYEVENTF_SCANCODE; [extended] = tiền tố E0. */
    data class Key(val scanCode: Int, val extended: Boolean, val down: Boolean) : RemoteInput

    /** Văn bản Unicode (host dùng KEYEVENTF_UNICODE). */
    data class Text(val text: String) : RemoteInput

    /** Bút: pressure 0..1024, tilt -90..90 độ. */
    data class Pen(
        val nx: Float,
        val ny: Float,
        val pressure: Int,
        val tiltX: Int,
        val tiltY: Int,
        val inRange: Boolean,
        val inContact: Boolean,
        val barrelButton: Boolean,
        val eraser: Boolean,
    ) : RemoteInput

    /**
     * Một điểm chạm của cảm ứng đa điểm (tối đa 10). x, y tuyệt đối chuẩn hóa 0..65535 trên màn hình host;
     * pressure 0..1024. Host chuyển thành POINTER_TOUCH_INFO (không phải chuột).
     */
    data class Touch(
        val id: Int,
        val x: Int,
        val y: Int,
        val phase: TouchPhase,
        val pressure: Int,
    ) : RemoteInput

    /** Ctrl+Alt+Del: host gọi SendSAS(FALSE) (không thể giả lập bằng SendInput). */
    data object SecureAttention : RemoteInput
}

fun interface InputSink {
    fun send(event: RemoteInput)
}
