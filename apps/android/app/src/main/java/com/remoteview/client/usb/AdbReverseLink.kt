package com.remoteview.client.usb

import android.util.Log
import java.net.InetSocketAddress
import java.net.Socket

// UNBUILT: chưa biên dịch.

/**
 * Dự phòng: host chạy `adb reverse tcp:47810 tcp:47810`, điện thoại kết nối 127.0.0.1:47810.
 * Cùng khung u32 big-endian như AOA. Cần bật Gỡ lỗi USB.
 */
class AdbReverseLink : FramedStreamLink("adb") {

    @Volatile private var socket: Socket? = null

    override fun start() {
        Thread({
            try {
                val s = Socket()
                s.tcpNoDelay = true
                s.connect(InetSocketAddress(HOST, PORT), CONNECT_TIMEOUT_MS)
                socket = s
                runStreams(s.getInputStream(), s.getOutputStream())
            } catch (e: Exception) {
                Log.w(TAG, "kết nối ADB reverse lỗi: ${e.message}")
                fail(e.message)
            }
        }, "rv-adb-open").also { it.isDaemon = true; it.start() }
    }

    override fun closeResources() {
        try { socket?.close() } catch (_: Exception) {}
        socket = null
    }

    companion object {
        private const val TAG = "RvAdb"
        const val HOST = "127.0.0.1"
        const val PORT = 47810
        private const val CONNECT_TIMEOUT_MS = 500

        /** Thăm dò nhanh (gọi ở thread nền): host đã `adb reverse` và đang lắng nghe chưa? */
        fun probe(timeoutMs: Int = 300): Boolean = try {
            Socket().use { it.connect(InetSocketAddress(HOST, PORT), timeoutMs); true }
        } catch (_: Exception) {
            false
        }
    }
}
