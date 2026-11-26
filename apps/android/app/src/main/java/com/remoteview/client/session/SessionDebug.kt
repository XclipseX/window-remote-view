package com.remoteview.client.session

/** Ảnh chụp trạng thái phiên cho lớp phủ gỡ lỗi. */
data class DebugSnapshot(
    val linkType: String = "-",
    val linkOpen: Boolean = false,
    val aoaOpened: String = "-",
    val bytesIn: Long = 0,
    val bytesOut: Long = 0,
    val datagramsIn: Long = 0,
    val datagramsOut: Long = 0,
    val handshake: String = "-",
    val nativeState: Int = -1,
    val nativeMessage: String = "",
    val nativeHandle: Long = 0,
    val decoderName: String = "-",
    val framesSubmitted: Long = 0,
    val framesDecoded: Long = 0,
    val lastError: String = "",
    /** SystemClock.elapsedRealtime() lúc state ESTABLISHED lần đầu; 0 nếu chưa. */
    val establishedAtMs: Long = 0,
)
