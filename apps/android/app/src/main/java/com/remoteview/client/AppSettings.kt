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
    /** Tự kết nối ngay khi cắm cáp (host AOA). */
    val autoConnectUsb: Boolean = true,
    /** Cho phép lớp phủ gỡ lỗi (hiện khi chưa có hình). */
    val debugOverlay: Boolean = true,
    /** Tự kiểm tra bản cập nhật khi mở app (tối đa 6 giờ một lần). */
    val autoCheckUpdate: Boolean = true,
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
            autoConnectUsb = prefs.getBoolean("usb_auto", d.autoConnectUsb),
            debugOverlay = prefs.getBoolean("dbg_overlay", d.debugOverlay),
            autoCheckUpdate = prefs.getBoolean("upd_auto", d.autoCheckUpdate),
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
            .putBoolean("usb_auto", s.autoConnectUsb)
            .putBoolean("dbg_overlay", s.debugOverlay)
            .putBoolean("upd_auto", s.autoCheckUpdate)
            .apply()
    }
}

/** Danh sách máy đã kết nối gần đây (tối đa 8), lưu dạng "tên|địa chỉ" mỗi dòng. */
class RecentHostsRepository(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences("recent_hosts", Context.MODE_PRIVATE)

    fun load(): List<Pair<String, String>> =
        (prefs.getString("list", "") ?: "").lines().mapNotNull { line ->
            val i = line.indexOf('|')
            if (i <= 0 || i == line.length - 1) null else line.substring(0, i) to line.substring(i + 1)
        }

    fun add(name: String, address: String): List<Pair<String, String>> {
        val cleanName = name.replace('|', ' ').replace('\n', ' ')
        val cleanAddr = address.replace('|', ' ').replace('\n', ' ')
        val list = (listOf(cleanName to cleanAddr) + load().filter { it.second != cleanAddr }).take(8)
        prefs.edit().putString("list", list.joinToString("\n") { "${it.first}|${it.second}" }).apply()
        return list
    }
}
