package com.remoteview.client.bench

import android.content.Context
import android.graphics.ImageFormat
import android.hardware.HardwareBuffer
import android.media.ImageReader
import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.Process
import android.util.Log
import android.view.Surface
import com.remoteview.client.decoder.LowLatencyDecoder
import com.remoteview.client.decoder.Vendor
import com.remoteview.client.perf.AdpfSession
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.locks.LockSupport

// UNBUILT: chưa biên dịch.

private const val TAG = "RvBenchDec"

/** Bộ ghi độ trễ (ns) thread-safe; drain() lấy và xóa các mẫu từ lần trước. */
class LatRec {
    private var a = LongArray(4096)
    private var n = 0
    @Synchronized fun add(v: Long) {
        if (n == a.size) a = a.copyOf(n * 2)
        a[n++] = v
    }
    @Synchronized fun drain(): LongArray {
        val r = a.copyOf(n); n = 0; return r
    }
}

/** Một thể hiện giải mã: MediaCodec async -> ImageReader ngoài màn hình. */
class DecodeWorker(
    private val index: Int,
    private val ctx: Context,
    private val cfg: BenchConfig,
    private val clip: Clip,
) {
    val outCount = AtomicLong()
    val missed = AtomicLong()
    val latencies = LatRec()
    @Volatile var error: String? = null
    @Volatile var tierLabel = ""
    private val adpfNs = AtomicLong()
    private val adpfN = AtomicLong()

    private val avail = ArrayDeque<Int>()
    private val submit = ConcurrentHashMap<Long, Long>()
    private val seq = AtomicLong()
    private var pos = 0
    @Volatile private var feeding = false
    @Volatile private var running = false

    private var codecThread: HandlerThread? = null
    private var readerThread: HandlerThread? = null
    private var feeder: Thread? = null
    private var codec: MediaCodec? = null
    private var reader: ImageReader? = null
    private var adpf: AdpfSession? = null

    fun drainAdpfAvgMs(): Float {
        val n = adpfN.getAndSet(0)
        val ns = adpfNs.getAndSet(0)
        return if (n == 0L) Float.NaN else ns / n / 1e6f
    }

    private fun makeReader(priv: Boolean): ImageReader? = try {
        val r = if (priv) {
            ImageReader.newInstance(
                cfg.width, cfg.height, ImageFormat.PRIVATE, 4,
                HardwareBuffer.USAGE_GPU_SAMPLED_IMAGE or HardwareBuffer.USAGE_COMPOSER_OVERLAY,
            )
        } else {
            ImageReader.newInstance(cfg.width, cfg.height, ImageFormat.YUV_420_888, 4)
        }
        r.setOnImageAvailableListener({ rd ->
            try { rd.acquireLatestImage()?.close() } catch (_: Exception) {}
        }, Handler(readerThread!!.looper))
        r
    } catch (t: Throwable) {
        Log.w(TAG, "ImageReader (private=$priv) lỗi: $t")
        null
    }

    private fun tryCodec(surface: Surface): MediaCodec? {
        val vendor = Vendor.fromCodecName(cfg.decoderName)
        for (tier in LowLatencyDecoder.buildTiers(vendor, cfg.fps)) {
            var c: MediaCodec? = null
            try {
                c = MediaCodec.createByCodecName(cfg.decoderName)
                val supported: Set<String>? =
                    if (Build.VERSION.SDK_INT >= 31) c.supportedVendorParameters.toSet() else null
                val f: MediaFormat = tier.buildFormat(cfg.codec.mime, cfg.width, cfg.height, cfg.fps, supported)
                f.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, clip.maxFrameBytes + clip.maxFrameBytes / 4 + 65536)
                c.setCallback(Callback(), Handler(codecThread!!.looper))
                c.configure(f, surface, null, 0)
                c.start()
                tierLabel = tier.label
                return c
            } catch (e: Exception) {
                Log.w(TAG, "Tier ${tier.label} lỗi trên ${cfg.decoderName}: $e")
                runCatching { c?.release() }
            }
        }
        return null
    }

    fun start(): Boolean {
        codecThread = HandlerThread("bench-dec$index", Process.THREAD_PRIORITY_URGENT_DISPLAY).also { it.start() }
        readerThread = HandlerThread("bench-img$index").also { it.start() }
        running = true
        val privOptions = if (Build.VERSION.SDK_INT >= 29) listOf(true, false) else listOf(false)
        for (priv in privOptions) {
            val r = makeReader(priv) ?: continue
            val c = tryCodec(r.surface)
            if (c != null) {
                reader = r; codec = c
                break
            }
            runCatching { r.close() }
        }
        if (codec == null) {
            error = "Không khởi tạo được ${cfg.decoderName}"
            return false
        }
        // Gửi dữ liệu cấu hình (SPS/PPS/VPS/seq header) trước.
        val deadline = System.nanoTime() + 3_000_000_000L
        var idx: Int? = null
        while (idx == null && System.nanoTime() < deadline) {
            idx = synchronized(avail) { avail.removeFirstOrNull() }
            if (idx == null) Thread.sleep(2)
        }
        if (idx == null) { error = "Decoder không cấp input buffer"; return false }
        try {
            val c = codec!!
            if (clip.config.isNotEmpty()) {
                val b = c.getInputBuffer(idx)!!
                b.clear(); b.put(clip.config)
                c.queueInputBuffer(idx, 0, clip.config.size, 0, MediaCodec.BUFFER_FLAG_CODEC_CONFIG)
            } else {
                synchronized(avail) { avail.addFirst(idx) }
            }
        } catch (e: Exception) { error = "Gửi cấu hình lỗi: $e"; return false }

        val tids = ArrayList<Int>()
        tids.add(codecThread!!.threadId)
        tids.add(readerThread!!.threadId)
        if (cfg.mode != BenchMode.THROUGHPUT) {
            val latch = CountDownLatch(1)
            feeder = Thread({
                tids.add(Process.myTid()); latch.countDown()
                feedLoop()
            }, "bench-feed$index").also { it.start() }
            latch.await()
        }
        if (cfg.adpf) adpf = AdpfSession(ctx, tids.toIntArray(), cfg.fps)
        feeding = true
        if (cfg.mode == BenchMode.THROUGHPUT) pump()
        return true
    }

    private fun pump() {
        synchronized(avail) {
            while (feeding && avail.isNotEmpty()) {
                val i = avail.removeFirst()
                feedFrame(i)
            }
        }
    }

    private fun feedLoop() {
        val period = 1_000_000_000L / cfg.fps
        var next = System.nanoTime() + period
        while (running) {
            val wait = next - System.nanoTime()
            if (wait > 300_000L) LockSupport.parkNanos(wait - 200_000L)
            while (running && System.nanoTime() < next) { /* spin */ }
            if (!running) break
            if (feeding) {
                val i = synchronized(avail) { avail.removeFirstOrNull() }
                if (i != null) feedFrame(i) else missed.incrementAndGet()
            }
            next += period
            val now = System.nanoTime()
            if (now - next > 5 * period) next = now + period
        }
    }

    private fun feedFrame(idx: Int) {
        val c = codec ?: return
        try {
            val f = clip.frames[pos]
            val key = clip.keys[pos]
            pos = (pos + 1) % clip.frames.size
            val b = c.getInputBuffer(idx) ?: return
            b.clear()
            if (f.size > b.capacity()) { error = "Frame ${f.size} B > input buffer ${b.capacity()} B"; return }
            b.put(f)
            val pts = seq.getAndIncrement() * 1_000_000L / cfg.fps
            if (submit.size > 4000) submit.clear()
            submit[pts] = System.nanoTime()
            c.queueInputBuffer(idx, 0, f.size, pts, if (key) MediaCodec.BUFFER_FLAG_KEY_FRAME else 0)
        } catch (e: Exception) {
            if (running) error = "queueInputBuffer: $e"
        }
    }

    fun stop() {
        running = false
        feeding = false
        runCatching { feeder?.join(500) }
        runCatching { codec?.stop() }
        runCatching { codec?.release() }
        codec = null
        runCatching { reader?.close() }
        reader = null
        adpf?.close(); adpf = null
        codecThread?.quitSafely(); readerThread?.quitSafely()
    }

    private inner class Callback : MediaCodec.Callback() {
        override fun onInputBufferAvailable(codec: MediaCodec, i: Int) {
            synchronized(avail) { avail.addLast(i) }
            if (cfg.mode == BenchMode.THROUGHPUT && feeding) pump()
        }

        override fun onOutputBufferAvailable(codec: MediaCodec, i: Int, info: MediaCodec.BufferInfo) {
            val now = System.nanoTime()
            val isCfg = info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0
            try { codec.releaseOutputBuffer(i, !isCfg) } catch (_: Exception) {}
            if (isCfg) return
            outCount.incrementAndGet()
            val t0 = submit.remove(info.presentationTimeUs)
            if (t0 != null) {
                val lat = now - t0
                latencies.add(lat)
                adpf?.let { it.report(lat); adpfNs.addAndGet(lat); adpfN.incrementAndGet() }
            }
        }

        override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) {}

        override fun onError(codec: MediaCodec, e: MediaCodec.CodecException) {
            if (running) error = "CodecException: ${e.diagnosticInfo}"
        }
    }
}
