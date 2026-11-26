package com.remoteview.client

import android.content.Context
import com.remoteview.client.decoder.CodecPreference
import com.remoteview.client.input.TouchMode

// UNBUILT: chưa biên dịch.

data class StreamSettings(
    /** Tỉ lệ độ phân giải so với màn hình gốc, 25..100. */
    val resolutionPercent: Int = 100,
    /** 30 / 60 / 90 / 120. */
    val fps: Int = 120,
    /** 1..100 -> ánh xạ ra bitrate. */
    val qualityPercent: Int = 70,
    val codec: CodecPreference = CodecPreference.AUTO,
    val hardwareDecode: Boolean = true,
    val allowAv1: Boolean = false,
    val rawTyping: Boolean = false,
    val directPresent: Boolean = false,
    val touchMode: TouchMode = TouchMode.DIRECT,
    /** Phát âm thanh của host (48 kHz stereo). */
    val audioEnabled: Boolean = true,
    /** Hiện thanh phím ở chế độ Màn hình phụ (USB); mặc định ẩn. */
    val usbShowModifierBar: Boolean = false,
    /** PIN/OTP 6-9 chữ số của rv-host (in ra khi host khởi động, hoặc cố định bằng `--otp`). */
    val usbOtp: String = "",
) {
    /** Placeholder: 5..120 Mbps tuyến tính theo chất lượng (mặc định LAN 60–120 Mbps). */
    fun targetBitrateKbps(): Int = 5_000 + ((qualityPercent.coerceIn(1, 100) - 1) / 99f * 115_000).toInt()

    companion object {
        val FPS_OPTIONS = listOf(30, 60, 90, 120)
        fun isValidOtp(s: String) = s.length in 6..9 && s.all { it in '0'..'9' }
    }
}

class SettingsRepository(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences("stream_settings", Context.MODE_PRIVATE)

    fun load(): StreamSettings {
        val d = StreamSettings()
        return StreamSettings(
            resolutionPercent = prefs.getInt("res", d.resolutionPercent),
            fps = prefs.getInt("fps", d.fps),
            qualityPercent = prefs.getInt("quality", d.qualityPercent),
            codec = runCatching { CodecPreference.valueOf(prefs.getString("codec", null) ?: "") }
                .getOrDefault(d.codec),
            hardwareDecode = prefs.getBoolean("hw", d.hardwareDecode),
            allowAv1 = prefs.getBoolean("av1", d.allowAv1),
            rawTyping = prefs.getBoolean("raw", d.rawTyping),
            directPresent = prefs.getBoolean("direct", d.directPresent),
            touchMode = runCatching { TouchMode.valueOf(prefs.getString("touch", null) ?: "") }
                .getOrDefault(d.touchMode),
            audioEnabled = prefs.getBoolean("audio", d.audioEnabled),
            usbShowModifierBar = prefs.getBoolean("usb_mod_bar", d.usbShowModifierBar),
            usbOtp = prefs.getString("usb_otp", d.usbOtp) ?: d.usbOtp,
        )
    }

    fun save(s: StreamSettings) {
        prefs.edit()
            .putInt("res", s.resolutionPercent)
            .putInt("fps", s.fps)
            .putInt("quality", s.qualityPercent)
            .putString("codec", s.codec.name)
            .putBoolean("hw", s.hardwareDecode)
            .putBoolean("av1", s.allowAv1)
            .putBoolean("raw", s.rawTyping)
            .putBoolean("direct", s.directPresent)
            .putString("touch", s.touchMode.name)
            .putBoolean("audio", s.audioEnabled)
            .putBoolean("usb_mod_bar", s.usbShowModifierBar)
            .putString("usb_otp", s.usbOtp)
            .apply()
    }
}
