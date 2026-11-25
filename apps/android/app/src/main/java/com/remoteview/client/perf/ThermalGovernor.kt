package com.remoteview.client.perf

import android.content.Context
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager

// UNBUILT: chưa biên dịch.

/**
 * Theo dõi thermal headroom: < 0.75 full; 0.75–0.9 bitrate 70%; >= 0.9 hạ về 60 fps.
 * Có độ trễ (hysteresis 0.05) khi đi xuống để tránh nhấp nháy.
 */
class ThermalGovernor(context: Context, private val listener: Listener) {

    interface Listener {
        /** scale = 1.0 (bình thường) hoặc 0.7. */
        fun onBitrateScale(scale: Float)
        /** maxFps = 60 khi quá nóng, null = bỏ giới hạn. */
        fun onFpsCap(maxFps: Int?)
    }

    enum class Level { FULL, REDUCED_BITRATE, DROP_TO_60 }

    var level: Level = Level.FULL
        private set

    private val pm = context.getSystemService(Context.POWER_SERVICE) as PowerManager
    private val handler = Handler(Looper.getMainLooper())
    private var running = false

    private val poll = object : Runnable {
        override fun run() {
            if (!running) return
            evaluate(readHeadroom())
            handler.postDelayed(this, POLL_MS)
        }
    }

    fun start() {
        if (running) return
        running = true
        handler.post(poll)
    }

    fun stop() {
        running = false
        handler.removeCallbacks(poll)
    }

    /** Headroom 0..1+ (1.0 = bắt đầu throttle nặng). Dự báo 10 s; rơi về thermal status nếu không có. */
    private fun readHeadroom(): Float {
        if (Build.VERSION.SDK_INT >= 30) {
            val h = pm.getThermalHeadroom(FORECAST_S)
            if (!h.isNaN()) return h
        }
        if (Build.VERSION.SDK_INT >= 29) {
            return when (pm.currentThermalStatus) {
                PowerManager.THERMAL_STATUS_NONE, PowerManager.THERMAL_STATUS_LIGHT -> 0.5f
                PowerManager.THERMAL_STATUS_MODERATE -> 0.8f
                else -> 0.95f // SEVERE trở lên
            }
        }
        return 0f
    }

    internal fun evaluate(headroom: Float) {
        val target = when (level) {
            Level.FULL -> when {
                headroom >= T_DROP -> Level.DROP_TO_60
                headroom >= T_REDUCE -> Level.REDUCED_BITRATE
                else -> Level.FULL
            }
            Level.REDUCED_BITRATE -> when {
                headroom >= T_DROP -> Level.DROP_TO_60
                headroom < T_REDUCE - HYST -> Level.FULL
                else -> Level.REDUCED_BITRATE
            }
            Level.DROP_TO_60 -> when {
                headroom < T_REDUCE - HYST -> Level.FULL
                headroom < T_DROP - HYST -> Level.REDUCED_BITRATE
                else -> Level.DROP_TO_60
            }
        }
        if (target == level) return
        level = target
        when (target) {
            Level.FULL -> { listener.onFpsCap(null); listener.onBitrateScale(1.0f) }
            Level.REDUCED_BITRATE -> { listener.onFpsCap(null); listener.onBitrateScale(0.7f) }
            Level.DROP_TO_60 -> { listener.onBitrateScale(0.7f); listener.onFpsCap(60) }
        }
    }

    companion object {
        const val T_REDUCE = 0.75f
        const val T_DROP = 0.90f
        private const val HYST = 0.05f
        private const val FORECAST_S = 10
        private const val POLL_MS = 3_000L
    }
}
