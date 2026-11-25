package com.remoteview.client.audio

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.os.Process
import android.util.Log
import com.remoteview.client.NativeBridge

// UNBUILT: chưa biên dịch.

/**
 * Phát PCM 48 kHz stereo s16le độ trễ thấp qua AudioTrack (PERFORMANCE_MODE_LOW_LATENCY).
 * TODO(oboe): chuyển sang Oboe/AAudio qua JNI để chạm đường MMAP và hạ thêm vài ms.
 *
 * Jitter buffer: hàng đợi gói PCM, tiền nạp [targetMs]; vượt [maxMs] thì bỏ gói cũ nhất (drop-oldest)
 * để độ trễ không tích lũy. Opus được giải mã bởi lõi Rust qua [NativeBridge.decodeOpus];
 * thiếu thư viện native thì gói Opus bị bỏ qua (no-op), PCM thô vẫn phát được.
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
        /** Opus tối đa 120 ms/gói. */
        private const val MAX_OPUS_FRAMES = SAMPLE_RATE / 1000 * 120

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
    private val opusOut = ShortArray(MAX_OPUS_FRAMES * CHANNELS)

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
        val pcm: ByteArray = if (codec == CODEC_OPUS) {
            val frames = NativeBridge.decodeOpus(data, opusOut)
            if (frames <= 0) return // chưa có native / lỗi giải mã: bỏ gói
            shortsToBytes(opusOut, frames * CHANNELS)
        } else {
            data
        }
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
        track?.let {
            try { it.pause(); it.flush() } catch (_: Exception) {}
            it.release()
        }
        track = null
    }

    private fun shortsToBytes(s: ShortArray, count: Int): ByteArray {
        val out = ByteArray(count * 2)
        for (i in 0 until count) {
            out[2 * i] = (s[i].toInt() and 0xFF).toByte()
            out[2 * i + 1] = (s[i].toInt() shr 8).toByte()
        }
        return out
    }
}
