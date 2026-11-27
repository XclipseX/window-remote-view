package com.remoteview.client

import android.util.Log
import androidx.annotation.Keep
import com.remoteview.client.input.InputSink
import com.remoteview.client.input.MouseButtonId
import com.remoteview.client.input.RemoteInput

/**
 * Giao diện JNI tới lõi Rust (`librv_core.so`, crate `crates/android-core`): phiên Noise/E2EE, ghép gói video (FEC),
 * đóng gói input. Mỗi hàm `external` khớp một symbol `Java_com_remoteview_client_NativeBridge_<tên>`.
 * CI build bằng cargo-ndk -> app/src/main/jniLibs/arm64-v8a/librv_core.so.
 * Hỗ trợ cả đường USB ([nativeConnectLink]) và đường LAN UDP ([nativeConnect]).
 * Opus được giải mã trong Kotlin (AudioPlayer, MediaCodec), lõi Rust chỉ chuyển nguyên gói.
 * TODO: đổi frame video sang direct ByteBuffer để tránh sao chép ByteArray.
 */
@Keep
object NativeBridge {

    private const val TAG = "RvNative"

    /** Rust gọi ngược vào đây (từ thread của nó). */
    @Keep
    interface Callbacks {
        /** Frame Annex-B đã ghép đủ. flags: bit0 = keyframe, bit1 = codec config. */
        fun onVideoFrame(data: ByteArray, ptsUs: Long, flags: Int)
        fun onConnectionState(state: Int, message: String?)
        fun onHostDiscovered(name: String, address: String)
        fun onStats(rttMs: Float, bitrateKbps: Int, lossPercent: Float)

        /** Gói âm thanh từ host. codec: 0 = PCM s16le 48 kHz stereo, 1 = Opus. Mặc định bỏ qua. */
        fun onAudioPacket(data: ByteArray, ptsUs: Long, codec: Int) {}

        /** Hình con trỏ của host (RGBA 8888 không nhân alpha, w*h*4 byte); hotX/hotY là điểm nóng trong ảnh. */
        fun onCursorShape(w: Int, h: Int, hotX: Int, hotY: Int, rgba: ByteArray) {}

        /** Vị trí con trỏ host, x,y chuẩn hóa 0..65535 trên khung hình video. */
        fun onCursorPos(x: Int, y: Int, visible: Boolean) {}
    }

    const val FLAG_KEYFRAME = 1
    const val FLAG_CONFIG = 2

    /** Mã state của [Callbacks.onConnectionState]. */
    const val STATE_CONNECTING = 0
    const val STATE_ESTABLISHED = 1
    const val STATE_CLOSED = 2
    const val STATE_FAILED = 3
    /** Host im lặng 5 s; lõi tự bắt tay lại, state 1 sẽ đến khi nối lại được. */
    const val STATE_DISCONNECTED = 4
    const val STATE_DISPLAY_CONFIG = 10
    /** message = "<codec> WxH@fps <kbps>kbps", codec: h264|hevc|av1. */
    const val STATE_STREAM_CONFIG = 11
    /** Host yêu cầu client gửi file log mới nhất lên máy tính. */
    const val STATE_LOG_REQUEST = 12

    @Volatile var callbacks: Callbacks? = null

    /** false nếu chưa có librv_core.so: UI vẫn chạy, mọi lệnh native thành no-op. */
    val available: Boolean = try {
        System.loadLibrary("rv_core")
        true
    } catch (e: UnsatisfiedLinkError) {
        Log.w(TAG, "librv_core.so chưa có: ${e.message}")
        false
    }

    // --- Phiên (handle là con trỏ/ID do Rust cấp) ---
    external fun nativeConnect(target: String, configJson: String): Long
    external fun nativeDisconnect(handle: Long)
    external fun nativePauseVideo(handle: Long, paused: Boolean)
    external fun nativeRequestKeyFrame(handle: Long)
    external fun nativeSetBitrate(handle: Long, kbps: Int)
    external fun nativeSetMaxFps(handle: Long, fps: Int)
    /** Báo thời gian giải mã MediaCodec thực tế (microsecond) để lõi gửi qua FeedbackStats. */
    external fun nativeReportDecodeUs(handle: Long, decodeUs: Int)
    /** Yêu cầu host đổi chất lượng/độ phân giải/FPS trong phiên (width/height = 0 giữ nguyên). */
    external fun nativeSetStreamQuality(handle: Long, quality: Int, width: Int, height: Int, fps: Int)

    // --- Liên kết datagram qua USB (AOA / ADB reverse): Kotlin chuyển byte, Rust lo giao thức ---
    /**
     * Tạo phiên chạy trên liên kết datagram do Kotlin cấp (thay vì tự mở socket).
     * [helloJson] chứa caps gửi trong Hello (độ phân giải gốc, tần số quét, cảm ứng, âm thanh...).
     * [helloJson] cũng mang `"otp"` (PIN ghép đôi với host). Trả 0 nếu lỗi (PIN sai định dạng...).
     * Gói gửi đi được Rust trả ngược qua [onSendDatagramFromNative].
     */
    external fun nativeConnectLink(helloJson: String): Long
    /** Đẩy một datagram nhận từ liên kết USB vào lõi Rust. */
    external fun nativePushDatagram(handle: Long, data: ByteArray)

    // --- Khám phá LAN (mDNS) ---
    external fun nativeStartDiscovery()
    external fun nativeStopDiscovery()

    // --- Mạng: Network.getNetworkHandle() để Rust bind socket vào đường di động/USB ---
    external fun nativeSetCellularNetwork(networkHandle: Long)

    // --- Input (Rust tự đóng gói + mã hóa) ---
    external fun nativeSendKey(handle: Long, scanCode: Int, extended: Boolean, down: Boolean)
    external fun nativeSendText(handle: Long, text: String)
    external fun nativeSendMouseAbs(handle: Long, nx: Float, ny: Float)
    external fun nativeSendMouseRel(handle: Long, dx: Int, dy: Int)
    external fun nativeSendMouseButton(handle: Long, button: Int, down: Boolean)
    external fun nativeSendWheel(handle: Long, horizontal: Int, vertical: Int)
    external fun nativeSendPen(
        handle: Long, nx: Float, ny: Float, pressure: Int, tiltX: Int, tiltY: Int,
        inRange: Boolean, inContact: Boolean, barrel: Boolean, eraser: Boolean,
    )
    external fun nativeSendSas(handle: Long)
    /**
     * Cảm ứng đa điểm (không phải chuột). x,y tuyệt đối chuẩn hóa 0..65535; phase theo
     * [com.remoteview.client.input.TouchPhase] ordinal (DOWN, MOVE, UP, CANCEL); pressure 0..1024.
     */
    external fun nativeSendTouch(handle: Long, id: Int, x: Int, y: Int, phase: Int, pressure: Int)

    /** Gửi file log (UTF-8) lên máy tính qua phiên đang mở; [name] là tên file đề xuất. Trả true nếu đã gửi. */
    external fun nativeUploadLog(handle: Long, name: String, data: ByteArray): Boolean

    /** Rust yêu cầu gửi một datagram ra liên kết USB; [datagramSender] do SessionController đặt. */
    @Volatile var datagramSender: ((ByteArray) -> Unit)? = null

    // --- Hàm Rust gọi ngược (qua JNI) để đẩy dữ liệu lên Kotlin ---
    @JvmStatic @Keep
    fun onVideoFrameFromNative(data: ByteArray, ptsUs: Long, flags: Int) {
        callbacks?.onVideoFrame(data, ptsUs, flags)
    }

    @JvmStatic @Keep
    fun onConnectionStateFromNative(state: Int, message: String?) {
        callbacks?.onConnectionState(state, message)
    }

    @JvmStatic @Keep
    fun onHostDiscoveredFromNative(name: String, address: String) {
        callbacks?.onHostDiscovered(name, address)
    }

    @JvmStatic @Keep
    fun onSendDatagramFromNative(data: ByteArray) {
        datagramSender?.invoke(data)
    }

    @JvmStatic @Keep
    fun onAudioFromNative(data: ByteArray, ptsUs: Long, codec: Int) {
        callbacks?.onAudioPacket(data, ptsUs, codec)
    }

    @JvmStatic @Keep
    fun onCursorShapeFromNative(w: Int, h: Int, hotX: Int, hotY: Int, rgba: ByteArray) {
        callbacks?.onCursorShape(w, h, hotX, hotY, rgba)
    }

    @JvmStatic @Keep
    fun onCursorPosFromNative(x: Int, y: Int, visible: Boolean) {
        callbacks?.onCursorPos(x, y, visible)
    }

    @JvmStatic @Keep
    fun onStatsFromNative(rttMs: Float, bitrateKbps: Int, lossPercent: Float) {
        callbacks?.onStats(rttMs, bitrateKbps, lossPercent)
    }
}

/** Đẩy [RemoteInput] sang Rust; no-op khi chưa có thư viện native hoặc chưa có phiên. */
class NativeInputSink(private val handleProvider: () -> Long) : InputSink {
    override fun send(event: RemoteInput) {
        val h = handleProvider()
        if (!NativeBridge.available || h == 0L) return
        when (event) {
            is RemoteInput.MouseMoveAbs -> NativeBridge.nativeSendMouseAbs(h, event.nx, event.ny)
            is RemoteInput.MouseMoveRel -> NativeBridge.nativeSendMouseRel(h, event.dx, event.dy)
            is RemoteInput.MouseButton -> NativeBridge.nativeSendMouseButton(h, event.button.ordinal, event.down)
            is RemoteInput.MouseWheel -> NativeBridge.nativeSendWheel(h, event.horizontal, event.vertical)
            is RemoteInput.Key -> NativeBridge.nativeSendKey(h, event.scanCode, event.extended, event.down)
            is RemoteInput.Text -> NativeBridge.nativeSendText(h, event.text)
            is RemoteInput.Pen -> NativeBridge.nativeSendPen(
                h, event.nx, event.ny, event.pressure, event.tiltX, event.tiltY,
                event.inRange, event.inContact, event.barrelButton, event.eraser,
            )
            is RemoteInput.Touch -> NativeBridge.nativeSendTouch(
                h, event.id, event.x, event.y, event.phase.ordinal, event.pressure,
            )
            RemoteInput.SecureAttention -> NativeBridge.nativeSendSas(h)
        }
    }

    @Suppress("unused")
    private val buttonOrder = MouseButtonId.values() // thứ tự ordinal = LEFT, RIGHT, MIDDLE, X1, X2 (hợp đồng với Rust)
}
