package com.remoteview.client.usb

import android.app.Activity
import android.os.Build
import android.util.DisplayMetrics
import com.remoteview.client.StreamSettings
import org.json.JSONArray
import org.json.JSONObject
import kotlin.math.roundToInt

// UNBUILT: chưa biên dịch.

/** Độ phân giải gốc + tần số quét cao nhất ở độ phân giải đó (ví dụ 2800x1752@120). */
data class NativeDisplay(val width: Int, val height: Int, val refreshHz: Int)

object DisplayCaps {

    @Suppress("DEPRECATION")
    fun nativeDisplay(activity: Activity): NativeDisplay {
        val d = activity.windowManager.defaultDisplay
        val dm = DisplayMetrics()
        d.getRealMetrics(dm)
        var hz = d.refreshRate
        if (Build.VERSION.SDK_INT >= 23) {
            val cur = d.mode
            val best = d.supportedModes
                .filter { it.physicalWidth == cur.physicalWidth && it.physicalHeight == cur.physicalHeight }
                .maxOfOrNull { it.refreshRate }
            if (best != null) hz = maxOf(hz, best)
        }
        return NativeDisplay(dm.widthPixels, dm.heightPixels, hz.roundToInt())
    }

    /**
     * JSON caps cho Hello. Ở chế độ [usbDisplay], host dùng display_width/height/refresh để tạo
     * màn hình ảo khớp (IDD) thay vì chụp màn hình có sẵn.
     * TODO(rust-core): chốt tên trường với giao thức Hello.
     */
    fun helloJson(activity: Activity, settings: StreamSettings, usbDisplay: Boolean): String {
        val nd = nativeDisplay(activity)
        val k = if (usbDisplay) 1f else settings.resolutionPercent / 100f
        fun even(v: Float) = (v.toInt() / 2) * 2
        return JSONObject().apply {
            put("mode", if (usbDisplay) "usb_display" else "mirror")
            put("display_width", even(nd.width * k))
            put("display_height", even(nd.height * k))
            put("display_refresh_hz", if (usbDisplay) nd.refreshHz else settings.fps)
            put("create_virtual_display", usbDisplay)
            put("touch", JSONObject().put("max_points", 10).put("absolute_max", 65535))
            put("pen", true)
            put("audio", JSONObject().apply {
                put("enabled", settings.audioEnabled)
                put("sample_rate", 48000)
                put("channels", 2)
                put("codecs", JSONArray().put("opus").put("pcm_s16le"))
            })
        }.toString()
    }
}
