package com.remoteview.client

import android.util.Log
import androidx.annotation.Keep
import com.remoteview.client.input.InputSink
import com.remoteview.client.input.MouseButtonId
import com.remoteview.client.input.RemoteInput

// UNBUILT: chưa biên dịch. Chưa có librv_core.so; toàn bộ là khai báo giao diện JNI.

/**
 * Giao diện JNI tới lõi Rust (`librv_core.so`): mạng, khám phá LAN, ghép gói video, đóng gói input.
 *
 * TODO(rust-core): triển khai các hàm `external` bằng crate JNI (jni-rs) trong thư mục crates/,
 *   build bằng cargo-ndk cho aarch64-linux-android, đặt .so vào apps/android/app/jniLibs/arm64-v8a/.
 *   Tên hàm JNI: Java_com_remoteview_client_NativeBridge_<tên>.
 * TODO(rust-core): đổi frame video sang direct ByteBuffer để tránh sao chép ByteArray.
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
    }

    const val FLAG_KEYFRAME = 1
    const val FLAG_CONFIG = 2

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
            RemoteInput.SecureAttention -> NativeBridge.nativeSendSas(h)
        }
    }

    @Suppress("unused")
    private val buttonOrder = MouseButtonId.values() // thứ tự ordinal = LEFT, RIGHT, MIDDLE, X1, X2 (hợp đồng với Rust)
}
