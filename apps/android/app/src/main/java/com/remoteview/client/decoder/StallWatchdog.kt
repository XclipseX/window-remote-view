package com.remoteview.client.decoder

import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import android.util.Log

private const val TAG = "RvWatchdog"

/**
 * Decoder Stall Watchdog (research-2): khi input vẫn chảy mà 1 s không có output -> flush + xin IDR;
 * 5 s vẫn không có output -> tạo lại codec (leo bậc ladder).
 */
class StallWatchdog(private val host: Host) {

    interface Host {
        val lastInputMs: Long
        val lastOutputMs: Long
        fun flushDecoder()
        fun recreateDecoder()
        fun requestKeyFrame()
    }

    private var thread: HandlerThread? = null
    private var handler: Handler? = null

    private var stage = 0 // 0 bình thường, 1 đã flush
    private var reference = 0L // mốc để tính "không có output" sau flush/recreate

    private val tick = object : Runnable {
        override fun run() {
            check()
            handler?.postDelayed(this, POLL_MS)
        }
    }

    fun start() {
        if (thread != null) return
        thread = HandlerThread("rv-watchdog").also { it.start() }
        handler = Handler(thread!!.looper)
        reference = SystemClock.elapsedRealtime()
        handler?.postDelayed(tick, POLL_MS)
    }

    fun stop() {
        handler?.removeCallbacksAndMessages(null)
        thread?.quitSafely()
        thread = null
        handler = null
    }

    private fun check() {
        val now = SystemClock.elapsedRealtime()
        val inputFlowing = now - host.lastInputMs < INPUT_FLOWING_MS
        val lastOut = host.lastOutputMs
        if (lastOut > reference && stage != 0) {
            // Có output trở lại.
            stage = 0
        }
        if (!inputFlowing) {
            stage = 0
            reference = now
            return
        }
        val noOutputMs = now - maxOf(lastOut, reference)
        when {
            stage == 0 && noOutputMs >= FLUSH_AFTER_MS -> {
                Log.w(TAG, "Không có output ${noOutputMs}ms -> flush + IDR")
                stage = 1
                reference = now
                host.requestKeyFrame()
                host.flushDecoder()
            }
            stage == 1 && now - reference >= RECREATE_AFTER_MS - FLUSH_AFTER_MS -> {
                Log.w(TAG, "Vẫn không có output sau flush -> recreate")
                stage = 0
                reference = now
                host.recreateDecoder()
                host.requestKeyFrame()
            }
        }
    }

    companion object {
        private const val POLL_MS = 250L
        private const val INPUT_FLOWING_MS = 500L
        private const val FLUSH_AFTER_MS = 1_000L
        private const val RECREATE_AFTER_MS = 5_000L
    }
}
