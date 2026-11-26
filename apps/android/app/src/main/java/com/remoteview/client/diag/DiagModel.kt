package com.remoteview.client.diag

import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import androidx.core.content.ContextCompat

// UNBUILT: chưa biên dịch.

enum class Verdict { OK, WARN, FAIL, INFO }

/** Kết quả một mục chẩn đoán (đã là chuỗi tiếng Việt, sẵn sàng hiển thị/xuất). */
data class CheckResult(
    val id: String,
    val title: String,
    val verdict: Verdict,
    val headline: String,
    val details: List<String> = emptyList(),
    val explanation: String? = null,
    val fix: String? = null,
)

data class DiagReport(
    val timeMillis: Long,
    val summary: CheckResult,
    val checks: List<CheckResult>,
)

/** Ảnh chụp trạng thái USB từ broadcast USB_STATE (các extra là "de-facto", vắng mặt = không rõ). */
data class UsbSnapshot(
    val broadcastSeen: Boolean,
    val connected: Boolean?,
    val configured: Boolean?,
    /** Mọi extra kiểu boolean (mtp, ptp, rndis, ncm, adb, midi, accessory, ...) mà hệ thống gửi. */
    val extras: Map<String, Boolean>,
) {
    fun fn(name: String): Boolean? = extras[name]
    fun activeFunctions(): List<String> =
        extras.filter { it.value && it.key !in NON_FUNCTION_KEYS }.keys.toList()

    val dataFunctionActive: Boolean
        get() = DATA_FUNCTIONS.any { extras[it] == true }

    companion object {
        val NON_FUNCTION_KEYS = setOf("connected", "configured", "host_connected", "unlocked")
        val DATA_FUNCTIONS = listOf("mtp", "ptp", "rndis", "ncm", "midi", "accessory")
    }
}

/** Dữ kiện thu thập dần giữa các mục để dựng phần "Kết luận". */
class DiagFacts {
    @Volatile var wifiOnly: Boolean? = null
    @Volatile var usb: UsbSnapshot? = null
    @Volatile var accessoryAttached: Boolean = false
    @Volatile var adbEnabled: Boolean? = null
    @Volatile var tetherActive: Boolean = false
    @Volatile var adbReverseOpen: Boolean = false
    @Volatile var hostsFound: Int = 0
    @Volatile var spacedeskInstalled: Boolean = false
    @Volatile var chosenCodec: String? = null
}

object UsbStates {
    const val ACTION = "android.hardware.usb.action.USB_STATE"

    @Suppress("DEPRECATION")
    fun parse(i: Intent?): UsbSnapshot {
        val ex = i?.extras ?: return UsbSnapshot(i != null, null, null, emptyMap())
        val map = LinkedHashMap<String, Boolean>()
        for (k in ex.keySet().sorted()) {
            val v = try { ex.get(k) } catch (_: Exception) { null }
            if (v is Boolean) map[k] = v
        }
        return UsbSnapshot(
            broadcastSeen = true,
            connected = if (ex.containsKey("connected")) ex.getBoolean("connected") else null,
            configured = if (ex.containsKey("configured")) ex.getBoolean("configured") else null,
            extras = map,
        )
    }

    fun readSticky(ctx: Context): UsbSnapshot = try {
        parse(ctx.registerReceiver(null, IntentFilter(ACTION)))
    } catch (_: Exception) {
        UsbSnapshot(false, null, null, emptyMap())
    }
}

/** Lắng nghe cắm/rút USB trực tiếp. [onChange] có cờ `initial` = true cho broadcast sticky đầu tiên. */
class UsbStateMonitor(
    private val ctx: Context,
    private val onChange: (snap: UsbSnapshot, initial: Boolean) -> Unit,
) {
    private var registered = false
    private val receiver = object : android.content.BroadcastReceiver() {
        override fun onReceive(c: Context?, intent: Intent?) {
            if (intent?.action != UsbStates.ACTION) return
            onChange(UsbStates.parse(intent), isInitialStickyBroadcast)
        }
    }

    fun start() {
        if (registered) return
        try {
            ContextCompat.registerReceiver(
                ctx, receiver, IntentFilter(UsbStates.ACTION), ContextCompat.RECEIVER_NOT_EXPORTED,
            )
            registered = true
        } catch (_: Exception) {}
    }

    fun stop() {
        if (!registered) return
        try { ctx.unregisterReceiver(receiver) } catch (_: Exception) {}
        registered = false
    }
}
