package com.remoteview.client.render

import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.view.Choreographer
import com.remoteview.client.decoder.FrameReleaser
import java.util.concurrent.atomic.AtomicLong

// UNBUILT: chưa biên dịch.

/**
 * Trình chiếu "Latest-Frame Rendering":
 * - CHOREOGRAPHER: gom output của codec, mỗi vsync chỉ render khung MỚI NHẤT, các khung cũ
 *   trả về bằng releaseOutputBuffer(idx, false) (bỏ khung stale thay vì tích trễ).
 * - DIRECT: render ngay khi giải mã xong (releaseOutputBuffer(idx, System.nanoTime())), cắt trễ vsync.
 *
 * Tạo trên main thread. [onOutput] gọi từ thread callback của codec.
 */
class FramePresenter(
    private val releaser: FrameReleaser,
    @Volatile var mode: Mode = Mode.CHOREOGRAPHER,
    /** Gọi trên main thread khi hình đứng yên > [IDLE_MS] (true) hoặc có khung mới trở lại (false). */
    private val onIdleChanged: ((idle: Boolean) -> Unit)? = null,
) : Choreographer.FrameCallback {

    enum class Mode { CHOREOGRAPHER, DIRECT }

    val presented = AtomicLong()
    val droppedStale = AtomicLong()

    /** Số liệu FPS/1% low/variance của các khung thực sự được trả ra surface. */
    val stats = FrameStats()

    private val main = Handler(Looper.getMainLooper())
    private val lock = Any()
    private val pending = ArrayList<Int>(4)
    private var scheduled = false
    @Volatile private var lastFrameMs: Long = 0L
    @Volatile private var idle = false
    @Volatile private var idleWatchArmed = false

    private val idleRunnable: Runnable = object : Runnable {
        override fun run() {
            val elapsed = SystemClock.elapsedRealtime() - lastFrameMs
            if (elapsed >= IDLE_MS) {
                idleWatchArmed = false
                if (!idle) {
                    idle = true
                    onIdleChanged?.invoke(true)
                }
            } else {
                main.postDelayed(this, (IDLE_MS - elapsed).coerceAtLeast(100L))
            }
        }
    }

    /** Callback từ LowLatencyDecoder.onOutput. */
    fun onOutput(index: Int, @Suppress("UNUSED_PARAMETER") ptsUs: Long) {
        if (mode == Mode.DIRECT) {
            val now = System.nanoTime()
            releaser.releaseAt(index, now)
            presented.incrementAndGet()
            stats.record(now)
            noteFrame()
            return
        }
        synchronized(lock) {
            pending.add(index)
            if (!scheduled) {
                scheduled = true
                main.post { Choreographer.getInstance().postFrameCallback(this) }
            }
        }
    }

    override fun doFrame(frameTimeNanos: Long) {
        val batch: IntArray
        synchronized(lock) {
            scheduled = false
            if (pending.isEmpty()) return
            batch = pending.toIntArray()
            pending.clear()
        }
        for (i in 0 until batch.size - 1) {
            releaser.release(batch[i], false)
            droppedStale.incrementAndGet()
        }
        releaser.releaseAt(batch.last(), frameTimeNanos)
        presented.incrementAndGet()
        // frameTimeNanos cùng gốc với System.nanoTime và khớp vsync -> phản ánh nhịp hiển thị thật.
        stats.record(frameTimeNanos)
        noteFrame()
    }

    private fun noteFrame() {
        lastFrameMs = SystemClock.elapsedRealtime()
        if (idle) {
            idle = false
            main.post { onIdleChanged?.invoke(false) }
        }
        if (!idleWatchArmed) {
            idleWatchArmed = true
            main.postDelayed(idleRunnable, IDLE_MS)
        }
    }

    /** Trả hết khung đang chờ (gọi trước flush/recreate/stop). */
    fun clear() {
        val batch: IntArray
        synchronized(lock) {
            batch = pending.toIntArray()
            pending.clear()
        }
        for (i in batch) releaser.release(i, false)
    }

    fun stop() {
        clear()
        idleWatchArmed = false
        idle = false
        main.removeCallbacksAndMessages(null)
        // Choreographer.getInstance() cần Looper: nếu stop() bị gọi từ thread không phải main thì đẩy lên main.
        if (Looper.myLooper() == Looper.getMainLooper()) {
            Choreographer.getInstance().removeFrameCallback(this)
        } else {
            main.post { Choreographer.getInstance().removeFrameCallback(this) }
        }
    }

    companion object {
        const val IDLE_MS = 3000L
    }
}
