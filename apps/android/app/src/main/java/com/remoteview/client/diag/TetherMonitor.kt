package com.remoteview.client.diag

import android.content.Context
import android.os.SystemClock
import com.remoteview.client.R
import java.net.Inet4Address
import java.net.NetworkInterface
import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
import kotlin.coroutines.coroutineContext

// UNBUILT: chưa biên dịch.

/**
 * Theo dõi việc bật USB tethering: ghi mọi broadcast USB_STATE và thay đổi giao diện rndis*/ncm*/usb*
 * (thăm dò 300 ms), rồi đưa ra kết luận vì sao công tắc tự tắt.
 */
object TetherMonitor {
    private val IFACE = Regex("^(rndis|ncm|usb)\\d*$")
    private const val FN_KEYS = "rndis,ncm,mtp,ptp,adb,accessory,midi"

    private class UsbEv(val t: Long, val snap: UsbSnapshot)

    suspend fun run(context: Context, durationMs: Long = 20_000, onLine: (String) -> Unit): CheckResult {
        val ctx = context.applicationContext
        fun s(id: Int, vararg a: Any) = ctx.getString(id, *a)
        val t0 = SystemClock.elapsedRealtime()
        fun rel(t: Long) = "+%.1fs".format(java.util.Locale.US, (t - t0) / 1000.0)

        val usbEvents = java.util.Collections.synchronizedList(ArrayList<UsbEv>())
        val timeline = java.util.Collections.synchronizedList(ArrayList<String>())
        fun log(t: Long, text: String) {
            val line = "${rel(t)}  $text"
            timeline.add(line)
            onLine(line)
        }
        fun describe(sn: UsbSnapshot): String {
            val fns = FN_KEYS.split(',').joinToString(" ") { k -> "$k=${sn.fn(k)?.let { if (it) 1 else 0 } ?: "?"}" }
            return "USB_STATE connected=${sn.connected} configured=${sn.configured} $fns"
        }

        val monitor = UsbStateMonitor(ctx) { snap, initial ->
            val t = SystemClock.elapsedRealtime()
            usbEvents.add(UsbEv(t, snap))
            log(t, (if (initial) "[baseline] " else "") + describe(snap))
        }
        monitor.start()

        // Giao diện: tên -> chuỗi IPv4 (rỗng = chưa có IP)
        val ifaceFirstSeen = HashMap<String, Long>()
        val ifaceGone = HashMap<String, Long>()
        var everIp = false
        var last: Map<String, String> = emptyMap()
        try {
            val end = t0 + durationMs
            while (SystemClock.elapsedRealtime() < end) {
                coroutineContext.ensureActive()
                val cur = HashMap<String, String>()
                try {
                    NetworkInterface.getNetworkInterfaces()?.toList().orEmpty().forEach { ni ->
                        if (IFACE.matches(ni.name)) {
                            val v4 = ni.inetAddresses.toList().filterIsInstance<Inet4Address>().mapNotNull { it.hostAddress }
                            cur[ni.name] = v4.joinToString(",") + if (try { ni.isUp } catch (_: Exception) { false }) "" else " (down)"
                        }
                    }
                } catch (_: Exception) {}
                val now = SystemClock.elapsedRealtime()
                for ((n, ip) in cur) {
                    if (last[n] != ip) {
                        if (n !in ifaceFirstSeen) ifaceFirstSeen[n] = now
                        if (ip.isNotBlank() && !ip.startsWith(" ")) everIp = true
                        log(now, "IFACE $n ${if (ip.isBlank()) "(chưa có IPv4)" else ip}")
                    }
                }
                for (n in last.keys) if (n !in cur) { ifaceGone[n] = now; log(now, "IFACE $n biến mất") }
                last = cur
                delay(300)
            }
        } finally {
            monitor.stop()
        }

        // ---- phân tích
        val snaps = synchronized(usbEvents) { usbEvents.toList() }
        val connectedEver = snaps.any { it.snap.connected == true }
        val configuredEver = snaps.any { it.snap.configured == true }
        fun tetherOn(sn: UsbSnapshot) = sn.fn("rndis") == true || sn.fn("ncm") == true
        val onEv = snaps.firstOrNull { tetherOn(it.snap) }
        val offEv = if (onEv == null) null else snaps.firstOrNull { it.t > onEv.t && !tetherOn(it.snap) }
        val ifaceSeen = ifaceFirstSeen.isNotEmpty()
        val appeared = onEv != null || ifaceSeen
        val aliveMs = when {
            onEv != null && offEv != null -> offEv.t - onEv.t
            ifaceSeen && ifaceGone.isNotEmpty() -> ifaceGone.values.min() - ifaceFirstSeen.values.min()
            else -> null
        }
        val title = s(R.string.diag_c_tether)
        val details = synchronized(timeline) { timeline.toList() }.ifEmpty { listOf(s(R.string.diag_tm_no_events)) }
        return when {
            appeared && everIp -> CheckResult("tether", title, Verdict.OK, s(R.string.diag_tm_ok), details, s(R.string.diag_tm_ok_explain), null)
            appeared && aliveMs != null && aliveMs <= 3000 ->
                CheckResult("tether", title, Verdict.FAIL, s(R.string.diag_tm_driver), details, s(R.string.diag_tm_driver_explain), s(R.string.diag_tm_driver_fix))
            appeared ->
                CheckResult("tether", title, Verdict.WARN, s(R.string.diag_tm_noip), details, s(R.string.diag_tm_noip_explain), s(R.string.diag_tm_driver_fix))
            !connectedEver ->
                CheckResult("tether", title, Verdict.WARN, s(R.string.diag_tm_nocable), details, s(R.string.diag_tm_nocable_explain), s(R.string.diag_usb_plug_fix))
            !configuredEver && snaps.none { it.snap.dataFunctionActive } ->
                CheckResult("tether", title, Verdict.FAIL, s(R.string.diag_tm_powerOnly), details, s(R.string.diag_tm_powerOnly_explain), s(R.string.diag_usb_chargeonly_fix))
            else ->
                CheckResult("tether", title, Verdict.FAIL, s(R.string.diag_tm_rejected), details, s(R.string.diag_tm_rejected_explain), s(R.string.diag_tm_rejected_fix))
        }
    }

    fun windowsInfo(ctx: Context): CheckResult = CheckResult(
        "windows", ctx.getString(R.string.diag_c_windows), Verdict.INFO,
        ctx.getString(R.string.diag_win_headline),
        ctx.getString(R.string.diag_win_steps).split('\n'),
        ctx.getString(R.string.diag_win_explain), null,
    )
}
