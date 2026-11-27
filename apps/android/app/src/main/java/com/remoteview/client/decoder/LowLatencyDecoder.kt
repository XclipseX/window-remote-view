package com.remoteview.client.decoder

import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import android.util.Log
import android.view.Surface
import java.util.concurrent.atomic.AtomicLong

// UNBUILT: chưa biên dịch. Theo docs/research-2-ket-qua-gemini.md (mục A) và research-1 (Agent 4).

private const val TAG = "RvDecoder"

object MimeTypes {
    const val AVC = "video/avc"
    const val HEVC = "video/hevc"
    const val AV1 = "video/av01" // MediaFormat.MIMETYPE_VIDEO_AV1 chỉ có từ API 29
}

enum class Vendor {
    EXYNOS, MTK, QTI, GENERIC;

    companion object {
        fun fromCodecName(name: String): Vendor {
            val n = name.lowercase()
            return when {
                n.startsWith("c2.exynos") || n.startsWith("omx.exynos") || n.startsWith("omx.sec") -> EXYNOS
                n.startsWith("c2.mtk") || n.startsWith("omx.mtk") -> MTK
                n.startsWith("c2.qti") || n.startsWith("omx.qcom") -> QTI
                else -> GENERIC
            }
        }
    }
}

enum class CodecPreference { AUTO, H264, HEVC, AV1 }

/** Chính sách chọn codec. [allowAv1] là cờ theo thiết bị (chỉ bật khi đo < 8 ms @120fps). */
data class DecoderPolicy(
    val preference: CodecPreference = CodecPreference.AUTO,
    val allowAv1: Boolean = false,
    val preferHardware: Boolean = true,
    /** Khác null: chỉ dùng đúng mime này (host đã chọn codec qua STREAM_CONFIG), không rơi về codec khác. */
    val forcedMime: String? = null,
)

data class CodecPick(val name: String, val mime: String, val vendor: Vendor, val hardware: Boolean)

/** Một frame Annex-B (H.264/HEVC) hoặc OBU (AV1) đã ghép đủ, sẵn sàng nạp vào MediaCodec. */
class EncodedFrame(
    val data: ByteArray,
    val ptsUs: Long,
    val isKeyFrame: Boolean,
    val isCodecConfig: Boolean = false,
)

/** Cho phép presenter trả buffer đầu ra mà không cần biết về MediaCodec. */
interface FrameReleaser {
    fun release(index: Int, render: Boolean)
    fun releaseAt(index: Int, renderTimeNs: Long)
}

object CodecSelector {

    private fun isHardware(info: MediaCodecInfo): Boolean =
        if (Build.VERSION.SDK_INT >= 29) {
            info.isHardwareAccelerated && !info.isSoftwareOnly
        } else {
            val n = info.name.lowercase()
            !n.startsWith("omx.google.") && !n.startsWith("c2.android.") && !n.contains(".sw.")
        }

    /** Trả về danh sách mime theo thứ tự ưu tiên. */
    fun mimeOrder(pref: CodecPreference, allowAv1: Boolean): List<String> = when (pref) {
        CodecPreference.AUTO ->
            if (allowAv1) listOf(MimeTypes.AV1, MimeTypes.HEVC, MimeTypes.AVC)
            else listOf(MimeTypes.HEVC, MimeTypes.AVC)
        CodecPreference.H264 -> listOf(MimeTypes.AVC)
        CodecPreference.HEVC -> listOf(MimeTypes.HEVC, MimeTypes.AVC)
        CodecPreference.AV1 -> listOf(MimeTypes.AV1, MimeTypes.HEVC, MimeTypes.AVC)
    }

    fun pick(
        policy: DecoderPolicy,
        width: Int,
        height: Int,
        fps: Int,
        exclude: Set<String> = emptySet(),
    ): CodecPick? {
        val infos = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.filter { !it.isEncoder }
        for (mime in policy.forcedMime?.let { listOf(it) } ?: mimeOrder(policy.preference, policy.allowAv1)) {
            val cands = infos.filter { info ->
                info.name !in exclude &&
                    info.supportedTypes.any { it.equals(mime, ignoreCase = true) } &&
                    !isFakeHardwareAv1(info.name, mime) &&
                    // Tránh AV1 trên c2.mtk ở >= 90 fps (D9300+ ~18-25 ms/khung).
                    // Trừ khi người dùng chọn rõ AV1.
                    !(mime == MimeTypes.AV1 && Vendor.fromCodecName(info.name) == Vendor.MTK && fps >= 90 &&
                        policy.preference != CodecPreference.AV1)
            }
            if (cands.isEmpty()) continue
            val sorted = cands.sortedWith(
                compareBy<MediaCodecInfo>(
                    { if (isHardware(it) == policy.preferHardware) 0 else 1 },
                    { if (supportsLowLatency(it, mime)) 0 else 1 },
                    { if (supportsSize(it, mime, width, height, fps)) 0 else 1 },
                ),
            )
            val best = sorted.first()
            return CodecPick(best.name, mime, Vendor.fromCodecName(best.name), isHardware(best))
        }
        return null
    }

    /** Có bộ giải mã phần cứng cho [mime] (dùng để quảng bá codec trong Hello). */
    fun hasHardwareDecoder(mime: String, excludeMtk: Boolean = false): Boolean = runCatching {
        MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.any { info ->
            !info.isEncoder && isHardware(info) &&
                !(excludeMtk && Vendor.fromCodecName(info.name) == Vendor.MTK) &&
                info.supportedTypes.any { it.equals(mime, ignoreCase = true) } &&
                !isFakeHardwareAv1(info.name, mime)
        }
    }.getOrDefault(false)

    // unisoc.av1.decoder thực chất là dav1d phần mềm giả danh phần cứng (Meta 2025).
    private fun isFakeHardwareAv1(name: String, mime: String) =
        mime == MimeTypes.AV1 && name.lowercase().contains("unisoc")

    private fun supportsLowLatency(info: MediaCodecInfo, mime: String): Boolean =
        Build.VERSION.SDK_INT >= 30 && runCatching {
            info.getCapabilitiesForType(mime).isFeatureSupported(MediaCodecInfo.CodecCapabilities.FEATURE_LowLatency)
        }.getOrDefault(false)

    private fun supportsSize(info: MediaCodecInfo, mime: String, w: Int, h: Int, fps: Int): Boolean =
        runCatching {
            info.getCapabilitiesForType(mime).videoCapabilities
                ?.areSizeAndRateSupported(w, h, fps.toDouble()) ?: true
        }.getOrDefault(true)
}

data class DecoderInfo(val codecName: String, val mime: String, val vendor: Vendor, val tierLabel: String, val tierIndex: Int)

/**
 * Bộ giải mã MediaCodec chế độ async, độ trễ thấp, kèm thang fallback theo hãng SoC.
 *
 * - Mỗi bậc tạo MediaCodec + MediaFormat MỚI (không tái dùng format đã dính khóa lỗi).
 * - Khóa vendor.* được lọc bằng getSupportedVendorParameters() (API 31+).
 * - Frame Annex-B vào qua [submit]; output ra qua [onOutput] (thường là FramePresenter).
 */
class LowLatencyDecoder(
    private val width: Int,
    private val height: Int,
    private val fps: Int,
    private val policy: DecoderPolicy,
    private val listener: Listener,
) : FrameReleaser {

    interface Listener {
        /** Cần IDR mới từ host (sau flush/recreate/rớt khung). */
        fun onKeyFrameNeeded()
        fun onDecoderStarted(info: DecoderInfo) {}
        /** Không khởi tạo/khôi phục được: nên yêu cầu host hạ về H.264. */
        fun onFatal(reason: String) {}
    }

    /** Đặt bởi FramePresenter. Nếu null, khung được render ngay. Gọi từ thread callback của codec. */
    @Volatile var onOutput: ((index: Int, ptsUs: Long) -> Unit)? = null

    /** Gọi từ thread callback của codec khi đo được độ trễ giải mã phần cứng (microsecond). */
    @Volatile var onDecodeTiming: ((decodeUs: Int) -> Unit)? = null

    @Volatile var info: DecoderInfo? = null
        private set

    // Dùng bởi StallWatchdog (SystemClock.elapsedRealtime, ms).
    @Volatile var lastInputMs: Long = 0
        private set
    @Volatile var lastOutputMs: Long = 0
        private set
    /** Thời gian giải mã MediaCodec khung gần nhất (microsecond, kẹp 0..500_000). */
    @Volatile var lastDecodeUs: Int = 0
        private set
    /** Thời gian giải mã trung bình EWMA (alpha = 0.2, đơn vị ms). */
    @Volatile var avgDecodeMs: Float = 0f
        private set
    /** Linux TID của luồng callback MediaCodec (dùng để đăng ký với ADPF). */
    @Volatile var codecThreadTid: Int = 0
        private set
    val framesIn = AtomicLong()
    val framesOut = AtomicLong()
    val framesDropped = AtomicLong()

    private val lifecycleLock = Any()
    private val lock = Any() // bảo vệ hàng đợi
    private val frames = ArrayDeque<EncodedFrame>()
    private val pendingInputs = ArrayDeque<Int>()
    private var waitingForKey = true

    // Vòng đệm 64 khe không cấp phát (ptsUs -> System.nanoTime() lúc queueInputBuffer) để đo decode latency thật.
    private val timingLock = Any()
    private val ptsRing = LongArray(TIMING_RING_SIZE) { Long.MIN_VALUE }
    private val enqNsRing = LongArray(TIMING_RING_SIZE)
    private var timingWriteIdx = 0
    private var hasDecodeSample = false

    private var thread: HandlerThread? = null
    private var handler: Handler? = null
    @Volatile private var codec: MediaCodec? = null
    private var surface: Surface? = null
    private var minTier = 0 // tăng mỗi lần watchdog buộc recreate -> bỏ qua bậc đã treo

    // ---------------------------------------------------------------- vòng đời

    fun start(surface: Surface): Boolean {
        synchronized(lifecycleLock) {
            this.surface = surface
            if (thread == null) {
                val t = HandlerThread("rv-codec").also { it.start() }
                thread = t
                handler = Handler(t.looper)
                val tid = t.threadId
                codecThreadTid = if (tid > 0) tid else 0
            }
            return startLocked()
        }
    }

    private fun startLocked(): Boolean {
        val surf = surface ?: return false
        val failedNames = HashSet<String>()
        while (true) {
            val pick = CodecSelector.pick(policy, width, height, fps, failedNames)
            if (pick == null) break // hết ứng viên (phần mềm đã nằm sẵn trong danh sách sắp xếp)
            val tiers = buildTiers(pick.vendor, fps)
            for ((i, tier) in tiers.withIndex()) {
                if (i < minTier && i != tiers.lastIndex) continue
                val c = tryTier(pick, tier, surf)
                if (c != null) {
                    codec = c
                    val di = DecoderInfo(pick.name, pick.mime, pick.vendor, tier.label, i)
                    info = di
                    val now = SystemClock.elapsedRealtime()
                    lastInputMs = now
                    lastOutputMs = now
                    synchronized(lock) {
                        pendingInputs.clear(); frames.clear(); waitingForKey = true
                    }
                    Log.i(TAG, "Decoder started: $di")
                    listener.onDecoderStarted(di)
                    listener.onKeyFrameNeeded()
                    return true
                }
            }
            Log.w(TAG, "Tất cả bậc đều thất bại với ${pick.name}; thử codec khác")
            failedNames += pick.name
        }
        listener.onFatal("Mọi bộ giải mã đều thất bại")
        return false
    }

    private fun tryTier(pick: CodecPick, tier: Tier, surf: Surface): MediaCodec? {
        var c: MediaCodec? = null
        try {
            c = MediaCodec.createByCodecName(pick.name)
            val supported: Set<String>? =
                if (Build.VERSION.SDK_INT >= 31) c.supportedVendorParameters.toSet() else null
            val format = tier.buildFormat(pick.mime, width, height, fps, supported)
            val cb = Callback(c)
            c.setCallback(cb, handler)
            c.configure(format, surf, null, 0)
            // Không bao giờ kéo giãn: giữ tỉ lệ (SurfaceView đã được đặt đúng tỉ lệ luồng, nên vừa khít).
            runCatching { c.setVideoScalingMode(MediaCodec.VIDEO_SCALING_MODE_SCALE_TO_FIT) }
            c.start()
            Log.i(TAG, "Tier '${tier.label}' OK trên ${pick.name}")
            return c
        } catch (e: Exception) {
            Log.w(TAG, "Tier '${tier.label}' thất bại trên ${pick.name}: $e")
            runCatching { c?.release() }
            return null
        }
    }

    /** Dừng hẳn và giải phóng. */
    fun stop() {
        synchronized(lifecycleLock) {
            stopCodecLocked()
            thread?.quitSafely()
            thread = null
            handler = null
            codecThreadTid = 0
        }
    }

    private fun stopCodecLocked() {
        val c = codec
        codec = null
        if (c != null) {
            runCatching { c.stop() }
            runCatching { c.release() }
        }
        synchronized(lock) { pendingInputs.clear(); frames.clear() }
        clearTimingRing(resetStats = true)
    }

    /** Watchdog 1 s: flush (async mode phải start() lại sau flush). */
    fun flush() {
        synchronized(lifecycleLock) {
            val c = codec ?: return
            try {
                c.flush()
                synchronized(lock) { pendingInputs.clear(); frames.clear(); waitingForKey = true }
                clearTimingRing(resetStats = false)
                c.start()
                listener.onKeyFrameNeeded()
            } catch (e: Exception) {
                Log.w(TAG, "flush thất bại, recreate: $e")
                recreate(escalate = true)
            }
        }
    }

    /** Watchdog 5 s hoặc lỗi nặng: dựng lại codec. [escalate] bỏ qua bậc ladder hiện tại. */
    fun recreate(escalate: Boolean) {
        synchronized(lifecycleLock) {
            if (surface == null) return
            if (escalate) minTier = (info?.tierIndex ?: -1) + 1
            stopCodecLocked()
            startLocked()
        }
    }

    private fun clearTimingRing(resetStats: Boolean) {
        synchronized(timingLock) {
            ptsRing.fill(Long.MIN_VALUE)
            enqNsRing.fill(0L)
            timingWriteIdx = 0
            if (resetStats) {
                hasDecodeSample = false
                lastDecodeUs = 0
                avgDecodeMs = 0f
            }
        }
    }

    private fun recordEnqueueTime(ptsUs: Long, nowNs: Long) {
        val ts = if (nowNs == 0L) 1L else nowNs
        synchronized(timingLock) {
            val slot = timingWriteIdx and TIMING_RING_MASK
            ptsRing[slot] = ptsUs
            enqNsRing[slot] = ts
            timingWriteIdx++
        }
    }

    private fun recordDequeueTime(ptsUs: Long, nowNs: Long): Int {
        synchronized(timingLock) {
            var foundNs = 0L
            var i = timingWriteIdx - 1
            for (step in 0 until TIMING_RING_SIZE) {
                val slot = i and TIMING_RING_MASK
                if (ptsRing[slot] == ptsUs && enqNsRing[slot] != 0L) {
                    foundNs = enqNsRing[slot]
                    ptsRing[slot] = Long.MIN_VALUE
                    enqNsRing[slot] = 0L
                    break
                }
                i--
            }
            if (foundNs == 0L) return -1
            val decodeUs = ((nowNs - foundNs) / 1000L).coerceIn(0L, MAX_DECODE_US).toInt()
            lastDecodeUs = decodeUs
            val decodeMs = decodeUs / 1000f
            avgDecodeMs = if (!hasDecodeSample) {
                hasDecodeSample = true
                decodeMs
            } else {
                avgDecodeMs * (1f - DECODE_EWMA_ALPHA) + decodeMs * DECODE_EWMA_ALPHA
            }
            return decodeUs
        }
    }

    // ---------------------------------------------------------------- đầu vào

    fun submit(frame: EncodedFrame) {
        synchronized(lock) {
            if (frames.size >= MAX_QUEUE) {
                // Backlog: bỏ hết, đợi IDR (độ trễ quan trọng hơn liền mạch).
                framesDropped.addAndGet(frames.size.toLong())
                frames.clear()
                waitingForKey = true
                listener.onKeyFrameNeeded()
            }
            frames.addLast(frame)
            drainInputLocked()
        }
    }

    private fun drainInputLocked() {
        val c = codec ?: return
        while (pendingInputs.isNotEmpty() && frames.isNotEmpty()) {
            val f = frames.removeFirst()
            if (waitingForKey && !f.isKeyFrame && !f.isCodecConfig) {
                framesDropped.incrementAndGet()
                continue
            }
            val idx = pendingInputs.removeFirst()
            try {
                val buf = c.getInputBuffer(idx) ?: continue
                if (f.data.size > buf.capacity()) {
                    Log.w(TAG, "Frame ${f.data.size}B > input buffer ${buf.capacity()}B, bỏ")
                    pendingInputs.addFirst(idx)
                    waitingForKey = true
                    listener.onKeyFrameNeeded()
                    continue
                }
                buf.clear()
                buf.put(f.data)
                var flags = 0
                if (f.isCodecConfig) flags = flags or MediaCodec.BUFFER_FLAG_CODEC_CONFIG
                if (f.isKeyFrame) flags = flags or MediaCodec.BUFFER_FLAG_KEY_FRAME
                c.queueInputBuffer(idx, 0, f.data.size, f.ptsUs, flags)
                if ((flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG) == 0) {
                    recordEnqueueTime(f.ptsUs, System.nanoTime())
                }
                if (f.isKeyFrame) waitingForKey = false
                lastInputMs = SystemClock.elapsedRealtime()
                framesIn.incrementAndGet()
            } catch (e: IllegalStateException) {
                Log.w(TAG, "queueInputBuffer: $e")
                return
            } catch (e: MediaCodec.CodecException) {
                Log.w(TAG, "queueInputBuffer codec exception: $e")
                return
            }
        }
    }

    // ---------------------------------------------------------------- đầu ra

    override fun release(index: Int, render: Boolean) {
        try { codec?.releaseOutputBuffer(index, render) } catch (_: IllegalStateException) {}
    }

    override fun releaseAt(index: Int, renderTimeNs: Long) {
        try { codec?.releaseOutputBuffer(index, renderTimeNs) } catch (_: IllegalStateException) {}
    }

    private inner class Callback(private val owner: MediaCodec) : MediaCodec.Callback() {
        override fun onInputBufferAvailable(codec: MediaCodec, index: Int) {
            if (codec !== this@LowLatencyDecoder.codec) return
            synchronized(lock) {
                pendingInputs.addLast(index)
                drainInputLocked()
            }
        }

        override fun onOutputBufferAvailable(codec: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
            if (codec !== this@LowLatencyDecoder.codec) return
            if (info.size == 0 || (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG) != 0) {
                runCatching { codec.releaseOutputBuffer(index, false) }
                return
            }
            val decodeUs = recordDequeueTime(info.presentationTimeUs, System.nanoTime())
            if (decodeUs >= 0) {
                onDecodeTiming?.invoke(decodeUs)
            }
            lastOutputMs = SystemClock.elapsedRealtime()
            framesOut.incrementAndGet()
            val sink = onOutput
            if (sink == null) {
                runCatching { codec.releaseOutputBuffer(index, true) }
            } else {
                sink(index, info.presentationTimeUs)
            }
        }

        override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) {
            Log.i(TAG, "Output format: $format")
        }

        override fun onError(codec: MediaCodec, e: MediaCodec.CodecException) {
            if (codec !== this@LowLatencyDecoder.codec) return
            Log.e(TAG, "CodecException transient=${e.isTransient} recoverable=${e.isRecoverable}: $e")
            if (e.isTransient) return
            handler?.post {
                if (e.isRecoverable) flush() else recreate(escalate = true)
            }
        }
    }

    // ---------------------------------------------------------------- thang fallback

    /**
     * Một bậc cấu hình. [intKeys] là khóa vendor (lọc theo getSupportedVendorParameters()).
     * [operatingRate] = null nghĩa là KHÔNG đặt KEY_OPERATING_RATE (bắt buộc với Exynos).
     */
    class Tier(
        val label: String,
        val standardLowLatency: Boolean,
        val operatingRate: Int?,
        val intKeys: List<Pair<String, Int>>,
    ) {
        /** Luôn tạo MediaFormat mới cho mỗi bậc. */
        fun buildFormat(mime: String, w: Int, h: Int, fps: Int, supported: Set<String>?): MediaFormat {
            val f = MediaFormat.createVideoFormat(mime, w, h)
            f.setInteger(MediaFormat.KEY_FRAME_RATE, fps)
            f.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, MAX_INPUT_SIZE)
            if (Build.VERSION.SDK_INT >= 23) f.setInteger(MediaFormat.KEY_PRIORITY, 0) // realtime
            if (standardLowLatency && Build.VERSION.SDK_INT >= 30) {
                f.setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            }
            if (operatingRate != null && Build.VERSION.SDK_INT >= 23) {
                f.setInteger(MediaFormat.KEY_OPERATING_RATE, operatingRate)
            }
            for ((key, value) in intKeys) {
                if (supported != null && key !in supported) {
                    Log.i(TAG, "Bỏ khóa không được hỗ trợ: $key")
                    continue
                }
                f.setInteger(key, value)
            }
            return f
        }
    }

    companion object {
        private const val MAX_QUEUE = 8
        private const val MAX_INPUT_SIZE = 4 * 1024 * 1024
        private const val TIMING_RING_SIZE = 64
        private const val TIMING_RING_MASK = TIMING_RING_SIZE - 1
        private const val MAX_DECODE_US = 500_000L
        private const val DECODE_EWMA_ALPHA = 0.2f

        private val PLAIN = Tier("plain", standardLowLatency = false, operatingRate = null, intKeys = emptyList())

        /** Thang bậc theo research-2: mỗi bậc là một MediaFormat mới. */
        fun buildTiers(vendor: Vendor, fps: Int): List<Tier> = when (vendor) {
            Vendor.EXYNOS -> listOf(
                // KHÔNG đặt KEY_OPERATING_RATE trên Exynos (Moonlight: crash/treo).
                Tier(
                    "exynos-0", true, null,
                    listOf(
                        "vendor.rtc-ext-dec-low-latency.enable" to 1,
                        "vendor.rtc-ext-dec-output-queue-depth.value" to 2,
                        "vendor.sec-dec-output.delay" to 0,
                    ),
                ),
                Tier("exynos-1", true, null, listOf("vendor.rtc-ext-dec-low-latency.enable" to 1)),
                Tier("exynos-2", false, null, listOf("vendor.rtc-ext-dec-low-latency.enable" to 1)),
                PLAIN,
            )
            Vendor.MTK -> listOf(
                Tier(
                    "mtk-0", true, fps,
                    listOf(
                        "vendor.mtk-codec2.low-latency-mode" to 1,
                        "vendor.mtk-codec2.game-mode" to 1,
                        "vendor.low-latency.enable" to 1,
                    ),
                ),
                // bậc 1: bỏ mtk-codec2.*
                Tier("mtk-1", true, fps, listOf("vendor.low-latency.enable" to 1)),
                // bậc 2: bỏ cả KEY_LOW_LATENCY
                Tier("mtk-2", false, fps, listOf("vendor.low-latency.enable" to 1)),
                PLAIN,
            )
            Vendor.QTI -> listOf(
                Tier(
                    "qti-0", true, null,
                    listOf(
                        "vendor.qti-ext-dec-low-latency.enable" to 1,
                        "vendor.qti-ext-dec-picture-order.enable" to 1,
                    ),
                ),
                Tier("qti-1", true, null, emptyList()),
                PLAIN,
            )
            Vendor.GENERIC -> listOf(
                Tier("std-low-latency", true, null, emptyList()),
                PLAIN,
            )
        }
    }
}

/** Tiện ích nhận dạng khung Annex-B (H.264/HEVC) nếu lõi mạng không báo cờ keyframe. */
object AnnexB {
    /** @return true nếu buffer chứa IDR/CRA/BLA (HEVC) hoặc IDR (H.264). */
    fun containsKeyFrame(data: ByteArray, mime: String): Boolean {
        var i = 0
        val n = data.size
        while (i + 3 < n) {
            if (data[i].toInt() == 0 && data[i + 1].toInt() == 0) {
                val sc3 = data[i + 2].toInt() == 1
                val sc4 = data[i + 2].toInt() == 0 && i + 4 < n && data[i + 3].toInt() == 1
                if (sc3 || sc4) {
                    val h = i + if (sc3) 3 else 4
                    if (h >= n) return false
                    val b = data[h].toInt() and 0xFF
                    if (mime == MimeTypes.HEVC) {
                        val type = (b shr 1) and 0x3F
                        if (type in 16..21) return true // BLA/IDR/CRA
                    } else {
                        if ((b and 0x1F) == 5) return true // IDR
                    }
                    i = h
                    continue
                }
            }
            i++
        }
        return false
    }
}
