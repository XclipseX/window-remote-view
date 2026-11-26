package com.remoteview.client.bench

import android.media.Image
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat
import android.util.Log
import java.util.Random

// UNBUILT: chưa biên dịch.

private const val TAG = "RvBenchClip"

class ClipException(message: String) : Exception(message)

/** Clip đã mã hóa (access unit trong RAM). frames[0] là IDR; không có B-frame. */
class Clip(
    val mime: String,
    val width: Int,
    val height: Int,
    val fps: Int,
    val bitrateBps: Int,
    val encoderName: String,
    val config: ByteArray,
    val frames: Array<ByteArray>,
    val keys: BooleanArray,
) {
    val totalBytes: Long get() = frames.sumOf { it.size.toLong() }
    val maxFrameBytes: Int get() = frames.maxOf { it.size }
    val actualMbps: Double get() = totalBytes * 8.0 / (frames.size.toDouble() / fps) / 1e6
}

object ClipGenerator {
    private const val SECONDS = 2
    private val cache = HashMap<String, Clip>()

    @Synchronized
    fun get(
        mime: String, w: Int, h: Int, fps: Int, bitrateMbps: Int,
        progress: (done: Int, total: Int) -> Unit,
        cancelled: () -> Boolean,
    ): Clip {
        val key = "$mime/$w/$h/$fps/$bitrateMbps"
        cache[key]?.let { return it }
        val encoders = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos
            .filter { it.isEncoder && it.supportedTypes.any { t -> t.equals(mime, true) } }
            .sortedBy { if (BenchDevice.isHardware(it)) 0 else 1 }
        if (encoders.isEmpty()) {
            throw ClipException("Máy không có bộ mã hóa $mime (bỏ qua codec này).")
        }
        var last = "không rõ"
        for (info in encoders) {
            if (cancelled()) throw ClipException("Đã hủy.")
            try {
                // Một số encoder (MTK) bỏ qua bitrate yêu cầu. Thử lại: hạ bitrate yêu cầu theo tỉ lệ,
                // rồi đổi sang VBR, giữ clip gần mức mong muốn nhất (để so sánh công bằng giữa các máy).
                var req = bitrateMbps
                var best = encode(info, mime, w, h, fps, req, progress, cancelled, forceVbr = false)
                val attempts = listOf(false, false, true)
                for (vbr in attempts) {
                    val ratio = best.actualMbps / bitrateMbps
                    if (ratio in 0.67..1.5 || cancelled()) break
                    req = (req / ratio).toInt().coerceAtLeast(1)
                    Log.i(TAG, "${info.name}: bitrate thực ${best.actualMbps} Mbps, thử lại với $req Mbps vbr=$vbr")
                    val c = encode(info, mime, w, h, fps, req, progress, cancelled, forceVbr = vbr)
                    if (kotlin.math.abs(c.actualMbps - bitrateMbps) < kotlin.math.abs(best.actualMbps - bitrateMbps)) best = c
                }
                cache[key] = best
                return best
            } catch (e: ClipException) {
                throw e
            } catch (e: Exception) {
                Log.w(TAG, "Encoder ${info.name} thất bại: $e")
                last = "${info.name}: ${e.message}"
            }
        }
        throw ClipException("Không mã hóa được clip $mime ${w}x$h ($last).")
    }

    private fun encode(
        info: MediaCodecInfo, mime: String, w: Int, h: Int, fps: Int, bitrateMbps: Int,
        progress: (Int, Int) -> Unit, cancelled: () -> Boolean, forceVbr: Boolean = false,
    ): Clip {
        val caps = info.getCapabilitiesForType(mime)
        val vc = caps.videoCapabilities
        if (vc != null && !vc.isSizeSupported(w, h)) throw IllegalStateException("không hỗ trợ ${w}x$h")
        var bitrate = bitrateMbps.toLong() * 1_000_000L
        if (vc != null) bitrate = bitrate.coerceIn(vc.bitrateRange.lower.toLong(), vc.bitrateRange.upper.toLong())
        val fmt = MediaFormat.createVideoFormat(mime, w, h)
        fmt.setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible)
        fmt.setInteger(MediaFormat.KEY_BIT_RATE, bitrate.toInt())
        fmt.setInteger(MediaFormat.KEY_FRAME_RATE, fps)
        // 600 s bị encoder MTK coi là không hợp lệ và mã toàn khung IDR (~435 Mbps). 10 s > độ dài clip.
        fmt.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 10)
        fmt.setInteger("max-bframes", 0) // = MediaFormat.KEY_MAX_B_FRAMES (API 29); khóa lạ bị bỏ qua
        val ec = caps.encoderCapabilities
        if (ec != null) {
            val mode = if (!forceVbr && ec.isBitrateModeSupported(MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR))
                MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR
            else MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_VBR
            fmt.setInteger(MediaFormat.KEY_BITRATE_MODE, mode)
        }

        val total = fps * SECONDS
        val frames = ArrayList<ByteArray>(total)
        val keys = ArrayList<Boolean>(total)
        var config = ByteArray(0)
        val synth = FrameSynth(w, h)
        val c = MediaCodec.createByCodecName(info.name)
        try {
            c.configure(fmt, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            c.start()
            val bi = MediaCodec.BufferInfo()
            var fed = 0
            var eos = false
            var done = false
            var lastProgressMs = System.currentTimeMillis()
            while (!done) {
                if (cancelled()) throw ClipException("Đã hủy.")
                if (System.currentTimeMillis() - lastProgressMs > 20_000) throw IllegalStateException("encoder treo")
                if (!eos) {
                    val i = c.dequeueInputBuffer(2000)
                    if (i >= 0) {
                        if (fed < total) {
                            val img = c.getInputImage(i) ?: throw IllegalStateException("getInputImage = null")
                            synth.fill(img, fed)
                            c.queueInputBuffer(i, 0, w * h * 3 / 2, fed * 1_000_000L / fps, 0)
                            fed++
                        } else {
                            c.queueInputBuffer(i, 0, 0, fed * 1_000_000L / fps, MediaCodec.BUFFER_FLAG_END_OF_STREAM)
                            eos = true
                        }
                    }
                }
                val oi = c.dequeueOutputBuffer(bi, 2000)
                if (oi >= 0) {
                    lastProgressMs = System.currentTimeMillis()
                    val buf = c.getOutputBuffer(oi)
                    if (buf != null && bi.size > 0) {
                        buf.position(bi.offset)
                        buf.limit(bi.offset + bi.size)
                        val bytes = ByteArray(bi.size)
                        buf.get(bytes)
                        if (bi.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                            config += bytes
                        } else {
                            frames.add(bytes)
                            keys.add(bi.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0 || frames.size == 1)
                            progress(frames.size, total)
                        }
                    }
                    c.releaseOutputBuffer(oi, false)
                    if (bi.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) done = true
                }
            }
        } finally {
            runCatching { c.stop() }
            runCatching { c.release() }
        }
        if (frames.isEmpty()) throw IllegalStateException("không có khung nào")
        Log.i(TAG, "Clip ${info.name} $mime ${w}x$h: ${frames.size} khung, ${frames.sumOf { it.size } / 1024} KiB")
        return Clip(mime, w, h, fps, bitrate.toInt(), info.name, config, frames.toTypedArray(), keys.toBooleanArray())
    }
}

/**
 * Tạo ảnh YUV: dải nhiễu trắng cuộn dọc (khó nén -> bitrate cao), vạch tương phản cao, gradient trôi,
 * và bộ đếm khung 32 bit ở góc trên trái. Hàng được sao chép bằng put() hàng loạt nên nhanh.
 */
private class FrameSynth(private val w: Int, private val h: Int) {
    private val rows = 256
    private val tmplY = Array(rows) { ByteArray(2 * w) }
    private val tmplU = Array(rows) { ByteArray(w) }
    private val tmplV = Array(rows) { ByteArray(w) }
    private val noise = ByteArray(1 shl 22)
    private val counterRow = ByteArray(32 * 16)

    init {
        Random(12345).nextBytes(noise)
        val cw = maxOf(1, w / 2)
        for (r in 0 until rows) {
            for (x in 0 until 2 * w) {
                val xx = x % w
                val v = if ((r / 16) % 2 == 0) {
                    if ((((xx / 6) + (r / 4)) and 1) == 0) 235 else 16
                } else {
                    ((xx * 255 / w + r) and 255)
                }
                tmplY[r][x] = v.toByte()
            }
            for (x in 0 until w) {
                val xc = x % cw
                tmplU[r][x] = ((xc * 255 / cw + r * 2) and 255).toByte()
                tmplV[r][x] = (255 - ((xc * 255 / cw + r) and 255)).toByte()
            }
        }
    }

    fun fill(img: Image, f: Int) {
        val planes = img.planes
        val yb = planes[0].buffer
        val ys = planes[0].rowStride
        val shift = (f * 11) % w
        for (y in 0 until h) {
            yb.position(y * ys)
            val yy = y + f * 3
            if ((yy / 8) % 2 == 0) {
                val off = (((f * 7919L + y * 104729L) * 2654435761L) ushr 7 and 0x7fffffffL).toInt() % (noise.size - w)
                yb.put(noise, off, w)
            } else {
                yb.put(tmplY[yy % rows], shift, w)
            }
        }
        for (b in 0 until 32) {
            val v: Byte = if (((f shr b) and 1) == 1) -1 else 0
            java.util.Arrays.fill(counterRow, b * 16, b * 16 + 16, v)
        }
        for (y in 0 until minOf(16, h)) {
            yb.position(y * ys)
            yb.put(counterRow, 0, minOf(counterRow.size, w))
        }
        val cw = w / 2
        val ch = h / 2
        val cs = shift / 2
        for (p in 1..2) {
            val buf = planes[p].buffer
            val rs = planes[p].rowStride
            val ps = planes[p].pixelStride
            val tm = if (p == 1) tmplU else tmplV
            for (y in 0 until ch) {
                val row = tm[(y + f) % rows]
                if (ps == 1) {
                    buf.position(y * rs)
                    buf.put(row, cs, cw)
                } else {
                    val base = y * rs
                    for (x in 0 until cw) buf.put(base + x * ps, row[cs + x])
                }
            }
        }
    }
}
