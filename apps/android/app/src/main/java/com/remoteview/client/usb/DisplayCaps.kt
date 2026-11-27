package com.remoteview.client.usb

import android.app.Activity
import android.os.Build
import android.util.DisplayMetrics
import com.remoteview.client.StreamSettings
import com.remoteview.client.decoder.CodecPreference
import com.remoteview.client.decoder.CodecSelector
import com.remoteview.client.decoder.MimeTypes
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
     * Danh sách codec quảng bá cho host theo thứ tự ưu tiên ("hevc"/"h264"/"av1").
     * HEVC chỉ khi có decoder phần cứng; AV1 chỉ khi cài đặt cho phép (hoặc người dùng chọn AV1) và có decoder phần cứng.
     * h264 luôn có mặt (mọi máy giải được) trừ khi bị lọc bởi lựa chọn codec... nó luôn là phương án cuối.
     */
    fun advertisedCodecs(settings: StreamSettings): List<String> {
        val hevc = CodecSelector.hasHardwareDecoder(MimeTypes.HEVC)
        // MediaTek (c2.mtk.*) giải AV1 chậm (~18-25 ms/khung): chỉ quảng bá khi người dùng CHỌN RÕ codec AV1.
        val explicitAv1 = settings.codec == CodecPreference.AV1
        val av1 = (settings.allowAv1 || explicitAv1) &&
            CodecSelector.hasHardwareDecoder(MimeTypes.AV1, excludeMtk = !explicitAv1)
        val out = ArrayList<String>()
        when (settings.codec) {
            CodecPreference.H264 -> {}
            CodecPreference.HEVC -> if (hevc) out += "hevc"
            CodecPreference.AV1 -> { if (av1) out += "av1"; if (hevc) out += "hevc" }
            CodecPreference.AUTO -> { if (av1) out += "av1"; if (hevc) out += "hevc" }
        }
        out += "h264"
        return out
    }

    /**
     * JSON caps cho Hello. Ở chế độ [usbDisplay], host dùng display_width/height/refresh để tạo
     * màn hình ảo khớp (IDD) thay vì chụp màn hình có sẵn.
     * Lõi Rust đọc display_width/height/refresh_hz (-> ControlMessage::Hello) và "otp" (PIN ghép đôi với host).
     */
    fun helloJson(
        activity: Activity,
        settings: StreamSettings,
        usbDisplay: Boolean,
        otp: String? = null,
        deviceName: String = DeviceName.friendly(activity),
    ): String {
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
            put("otp", otp ?: settings.usbOtp)
            put("device_name", deviceName)
            put("pen", true)
            put("codecs", JSONArray().also { a -> advertisedCodecs(settings).forEach { a.put(it) } })
            put("audio", JSONObject().apply {
                put("enabled", settings.audioEnabled)
                put("sample_rate", 48000)
                put("channels", 2)
                put("codecs", JSONArray().put("opus").put("pcm_s16le"))
            })
        }.toString()
    }
}
