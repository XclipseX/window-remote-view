package com.remoteview.client.audio

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Process
import android.util.Log
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/**
 * Phát PCM 48 kHz stereo s16le độ trễ thấp qua AudioTrack (PERFORMANCE_MODE_LOW_LATENCY).
 * TODO(oboe): chuyển sang Oboe/AAudio qua JNI để chạm đường MMAP và hạ thêm vài ms.
 *
 * Jitter buffer: hàng đợi gói PCM, tiền nạp [targetMs]; vượt [maxMs] thì bỏ gói cũ nhất (drop-oldest)
 * để độ trễ không tích lũy. Opus (gói thô do lõi Rust chuyển lên) được giải mã bằng MediaCodec
 * "audio/opus" (c2.android.opus.decoder) ở thread riêng; không tạo được codec thì gói Opus bị bỏ, PCM thô vẫn phát.
 */
class AudioPlayer(
    private val targetMs: Int = 30,
    private val maxMs: Int = 40,
) {
    companion object {
        private const val TAG = "RvAudio"
        const val SAMPLE_RATE = 48_000
        const val CHANNELS = 2
        private const val BYTES_PER_FRAME = 2 * CHANNELS
        private const val OPUS_MIME = "audio/opus"
        private const val OPUS_PRE_SKIP = 312 // mẫu @48k (6,5 ms), giá trị mặc định của libopus
        private const val OPUS_QUEUE_LIMIT = 32 // gói; ~10 ms/gói -> bỏ gói cũ nếu giải mã không kịp

        const val CODEC_PCM = 0
        const val CODEC_OPUS = 1
    }

    private val lock = Object()
    private val queue = ArrayDeque<ByteArray>()
    private var queuedBytes = 0
    private var primed = false
    @Volatile private var running = false
    private var track: AudioTrack? = null
    private var thread: Thread? = null
    private val opusQueue = LinkedBlockingQueue<ByteArray>(OPUS_QUEUE_LIMIT)
    private var opusThread: Thread? = null
    private var opusCodec: MediaCodec? = null

    private fun bytesFor(ms: Int) = SAMPLE_RATE / 1000 * ms * BYTES_PER_FRAME

    fun start(): Boolean {
        if (running) return true
        return try {
            val format = AudioFormat.Builder()
                .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .setSampleRate(SAMPLE_RATE)
                .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
                .build()
            val attrs = AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_MEDIA)
                .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                .build()
            val min = AudioTrack.getMinBufferSize(
                SAMPLE_RATE, AudioFormat.CHANNEL_OUT_STEREO, AudioFormat.ENCODING_PCM_16BIT,
            )
            val bufBytes = maxOf(min, bytesFor(20))
            val t = AudioTrack.Builder()
                .setAudioAttributes(attrs)
                .setAudioFormat(format)
                .setTransferMode(AudioTrack.MODE_STREAM)
                .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
                .setBufferSizeInBytes(bufBytes)
                .build()
            track = t
            running = true
            t.play()
            thread = Thread({ playLoop(t) }, "rv-audio").also { it.isDaemon = true; it.start() }
            startOpus()
            true
        } catch (e: Exception) {
            Log.w(TAG, "không khởi tạo được AudioTrack: ${e.message}")
            track?.release(); track = null
            running = false
            false
        }
    }

    /** Nhận một gói từ lõi (thread bất kỳ). [codec] = [CODEC_PCM] hoặc [CODEC_OPUS]. */
    fun submit(data: ByteArray, codec: Int) {
        if (!running) return
        if (codec == CODEC_OPUS) {
            if (opusCodec == null) return // không có decoder: bỏ gói
            while (!opusQueue.offer(data)) opusQueue.poll() // drop-oldest
        } else {
            enqueuePcm(data)
        }
    }

    private fun enqueuePcm(pcm: ByteArray) {
        if (pcm.isEmpty()) return
        synchronized(lock) {
            queue.addLast(pcm)
            queuedBytes += pcm.size
            val max = bytesFor(maxMs)
            while (queuedBytes > max && queue.size > 1) {
                queuedBytes -= queue.removeFirst().size // drop-oldest
            }
            lock.notifyAll()
        }
    }

    // --- Opus (MediaCodec) ---

    private fun opusFormat(): MediaFormat {
        val fmt = MediaFormat.createAudioFormat(OPUS_MIME, SAMPLE_RATE, CHANNELS)
        val head = ByteBuffer.allocate(19).order(ByteOrder.LITTLE_ENDIAN)
        head.put("OpusHead".toByteArray(Charsets.US_ASCII))
        head.put(1) // version
        head.put(CHANNELS.toByte())
        head.putShort(OPUS_PRE_SKIP.toShort())
        head.putInt(SAMPLE_RATE)
        head.putShort(0) // output gain
        head.put(0) // channel mapping family 0 (stereo)
        head.flip()
        fun nsBuf(ns: Long) = ByteBuffer.allocate(8).order(ByteOrder.nativeOrder()).putLong(ns).also { it.flip() }
        fmt.setByteBuffer("csd-0", head)
        fmt.setByteBuffer("csd-1", nsBuf(OPUS_PRE_SKIP * 1_000_000_000L / SAMPLE_RATE)) // codec delay
        fmt.setByteBuffer("csd-2", nsBuf(80_000_000L)) // seek pre-roll
        return fmt
    }

    private fun startOpus() {
        try {
            val c = MediaCodec.createDecoderByType(OPUS_MIME)
            c.configure(opusFormat(), null, null, 0)
            c.start()
            opusCodec = c
            opusThread = Thread({ opusLoop(c) }, "rv-opus").also { it.isDaemon = true; it.start() }
        } catch (e: Exception) {
            Log.w(TAG, "không tạo được decoder Opus: ${e.message}")
            opusCodec?.let { try { it.release() } catch (_: Exception) {} }
            opusCodec = null
        }
    }

    private fun opusLoop(c: MediaCodec) {
        try { Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO) } catch (_: Exception) {}
        val info = MediaCodec.BufferInfo()
        var ptsUs = 0L
        try {
            while (running) {
                val pkt = opusQueue.poll(20, TimeUnit.MILLISECONDS)
                if (pkt != null) {
                    val idx = c.dequeueInputBuffer(5_000)
                    if (idx >= 0) {
                        val buf = c.getInputBuffer(idx)!!
                        buf.clear()
                        buf.put(pkt)
                        c.queueInputBuffer(idx, 0, pkt.size, ptsUs, 0)
                        ptsUs += 10_000 // chỉ để tăng đơn điệu; không dùng để đồng bộ
                    }
                }
                while (running) {
                    val out = c.dequeueOutputBuffer(info, 0)
                    if (out >= 0) {
                        if (info.size > 0) {
                            val ob = c.getOutputBuffer(out)!!
                            ob.position(info.offset)
                            ob.limit(info.offset + info.size)
                            val pcm = ByteArray(info.size)
                            ob.get(pcm)
                            enqueuePcm(pcm) // PCM16 little-endian xen kẽ 48 kHz stereo
                        }
                        c.releaseOutputBuffer(out, false)
                    } else break // INFO_TRY_AGAIN_LATER / INFO_OUTPUT_FORMAT_CHANGED: vòng sau
                }
            }
        } catch (e: Exception) {
            if (running) Log.w(TAG, "decoder Opus lỗi: ${e.message}")
        }
    }

    private fun playLoop(t: AudioTrack) {
        try { Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO) } catch (_: Exception) {}
        val prime = bytesFor(targetMs)
        while (running) {
            val chunk: ByteArray? = synchronized(lock) {
                if (!primed) {
                    // Tiền nạp tới ngưỡng jitter (hoặc chờ tối đa 50 ms để không treo).
                    if (queuedBytes < prime) lock.wait(50)
                    if (queuedBytes >= prime) primed = true
                }
                if (primed) {
                    if (queue.isEmpty()) lock.wait(20)
                    val c = queue.removeFirstOrNull()
                    if (c == null) primed = false // underrun: nạp lại
                    else queuedBytes -= c.size
                    c
                } else null
            }
            if (chunk != null) t.write(chunk, 0, chunk.size) // WRITE_BLOCKING mặc định
        }
    }

    fun stop() {
        running = false
        synchronized(lock) { queue.clear(); queuedBytes = 0; primed = false; lock.notifyAll() }
        try { thread?.join(200) } catch (_: InterruptedException) {}
        thread = null
        try { opusThread?.join(200) } catch (_: InterruptedException) {}
        opusThread = null
        opusQueue.clear()
        opusCodec?.let { try { it.stop() } catch (_: Exception) {}; try { it.release() } catch (_: Exception) {} }
        opusCodec = null
        track?.let {
            try { it.pause(); it.flush() } catch (_: Exception) {}
            it.release()
        }
        track = null
    }
}
