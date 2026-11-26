package com.remoteview.client.bench

import android.content.Context
import android.hardware.display.DisplayManager
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.os.Build
import android.view.Display
import com.remoteview.client.decoder.MimeTypes

// UNBUILT: chưa biên dịch. Màn hình "Kiểm tra giới hạn (Stress test)": đo giới hạn giải mã video và bóp xung nhiệt.

enum class BenchMode { THROUGHPUT, PACED, SUSTAINED }

enum class BenchCodec(val mime: String, val label: String) {
    H264(MimeTypes.AVC, "H.264"),
    HEVC(MimeTypes.HEVC, "HEVC"),
    AV1(MimeTypes.AV1, "AV1"),
}

data class BenchResolution(val width: Int, val height: Int, val native: Boolean = false) {
    val label: String get() = if (native) "Native ${width}x$height" else "${width}x$height"
}

data class BenchConfig(
    val mode: BenchMode,
    val codec: BenchCodec,
    val decoderName: String,
    val width: Int,
    val height: Int,
    val fps: Int,
    val durationSec: Int,
    val instances: Int,
    val bitrateMbps: Int,
    val adpf: Boolean,
    val allowCritical: Boolean,
)

/** Một mẫu 1 giây. Giá trị không đo được = NaN (hoặc -1 với số nguyên). */
data class BenchSample(
    val tSec: Int,
    val fps: Float,
    val p50Ms: Float,
    val p99Ms: Float,
    val maxMs: Float,
    val missed: Long,
    val headroom: Float,
    val thermalStatus: Int,
    val batteryPct: Int,
    val batteryTempC: Float,
    val currentMa: Float,
    val voltageMv: Int,
    val watts: Float,
    val refreshHz: Float,
    val adpfMs: Float,
    val plugged: Boolean,
)

class BenchSummary(val text: String, val csv: String, val throttled: Boolean?)

object BenchDevice {
    fun isHardware(info: MediaCodecInfo): Boolean =
        if (Build.VERSION.SDK_INT >= 29) {
            info.isHardwareAccelerated && !info.isSoftwareOnly
        } else {
            val n = info.name.lowercase()
            !n.startsWith("omx.google.") && !n.startsWith("c2.android.") && !n.contains(".sw.")
        }

    /** Mọi bộ giải mã phần cứng cho mime, gồm cả biến thể .lowlatency. */
    fun hardwareDecoders(mime: String): List<String> =
        MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos
            .filter { !it.isEncoder && isHardware(it) && it.supportedTypes.any { t -> t.equals(mime, true) } }
            .map { it.name }

    fun supportsSize(name: String, mime: String, w: Int, h: Int, fps: Int): Boolean =
        runCatching {
            val info = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.first { it.name == name }
            info.getCapabilitiesForType(mime).videoCapabilities.areSizeAndRateSupported(w, h, fps.toDouble())
        }.getOrDefault(true)

    fun nativeResolution(ctx: Context): BenchResolution {
        return runCatching {
            val dm = ctx.getSystemService(Context.DISPLAY_SERVICE) as DisplayManager
            val m = dm.getDisplay(Display.DEFAULT_DISPLAY).mode
            val w = maxOf(m.physicalWidth, m.physicalHeight) and 1.inv()
            val h = minOf(m.physicalWidth, m.physicalHeight) and 1.inv()
            BenchResolution(w, h, native = true)
        }.getOrDefault(BenchResolution(2800, 1752, native = true))
    }
}
