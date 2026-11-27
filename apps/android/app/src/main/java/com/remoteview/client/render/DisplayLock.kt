package com.remoteview.client.render

import android.app.Activity
import android.os.Build
import android.util.Log
import android.view.Surface
import android.view.WindowManager

// UNBUILT: chưa biên dịch.

/** Khóa tần số quét màn hình theo FPS nguồn và chọn mode refresh cao nhất ở độ phân giải gốc. */
object DisplayLock {
    private const val TAG = "RvDisplayLock"

    /** Surface.setFrameRate(fps, FIXED_SOURCE, CHANGE_FRAME_RATE_ALWAYS). */
    fun applyFrameRate(surface: Surface, fps: Float) {
        if (!surface.isValid) return
        try {
            when {
                Build.VERSION.SDK_INT >= 31 -> surface.setFrameRate(
                    fps,
                    Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE,
                    Surface.CHANGE_FRAME_RATE_ALWAYS,
                )
                Build.VERSION.SDK_INT >= 30 -> surface.setFrameRate(
                    fps,
                    Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE,
                )
            }
        } catch (e: IllegalArgumentException) {
            Log.w(TAG, "setFrameRate($fps) bị từ chối: $e")
        }
    }

    /** Màn hình Windows đứng yên: hạ nhịp xuống 60 Hz (tránh giật khi LTPO tăng tốc từ 10 Hz lên 120 Hz). */
    fun setIdle(surface: Surface, idle: Boolean, fps: Float) {
        applyFrameRate(surface, if (idle) minOf(fps, 60f) else fps)
    }

    /**
     * Chọn Display.Mode có refresh cao nhất ở cùng độ phân giải vật lý với mode hiện tại
     * và đặt WindowManager.LayoutParams.preferredDisplayModeId.
     * @return refresh rate đã chọn, hoặc null nếu không áp dụng được (API < 23).
     */
    fun lockHighestRefresh(activity: Activity): Float? {
        if (Build.VERSION.SDK_INT < 23) return null
        @Suppress("DEPRECATION")
        val display = if (Build.VERSION.SDK_INT >= 30) {
            runCatching { activity.display }.getOrNull()
        } else {
            activity.windowManager.defaultDisplay
        } ?: return null

        val cur = display.mode
        val best = display.supportedModes
            .filter { it.physicalWidth == cur.physicalWidth && it.physicalHeight == cur.physicalHeight }
            .maxByOrNull { it.refreshRate } ?: return null

        val lp: WindowManager.LayoutParams = activity.window.attributes
        lp.preferredDisplayModeId = best.modeId
        activity.window.attributes = lp
        Log.i(TAG, "preferredDisplayModeId=${best.modeId} (${best.refreshRate} Hz)")
        return best.refreshRate
    }
}
