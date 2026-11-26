package com.remoteview.client.diag

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import com.remoteview.client.R
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import kotlinx.coroutines.ensureActive
import kotlin.coroutines.coroutineContext

// UNBUILT: chưa biên dịch.

object DiagRunner {

    /** Chạy toàn bộ kiểm tra (gọi trong Dispatchers.IO). [onEach] nhận từng kết quả ngay khi xong. */
    suspend fun runAll(context: Context, onEach: (CheckResult) -> Unit): DiagReport {
        val ctx = context.applicationContext
        val facts = DiagFacts()
        val c = DiagChecks(ctx, facts)
        val steps: List<Pair<String, () -> CheckResult>> = listOf(
            "device" to c::device,
            "cellular" to c::cellular,
            "usb" to c::usbState,
            "aoa" to c::accessory,
            "dev" to c::developer,
            "net" to c::network,
            "reach" to c::reachability,
            "apps" to c::relatedApps,
            "codec" to c::codecs,
            "display" to c::display,
            "thermal" to c::thermalPower,
            "audio" to c::audio,
        )
        val out = ArrayList<CheckResult>()
        for ((id, fn) in steps) {
            coroutineContext.ensureActive()
            val r = try {
                fn()
            } catch (t: Throwable) {
                CheckResult(
                    id, id, Verdict.WARN,
                    ctx.getString(R.string.diag_check_crashed),
                    listOf(t.javaClass.simpleName + ": " + (t.message ?: "")),
                )
            }
            out += r
            onEach(r)
        }
        val win = TetherMonitor.windowsInfo(ctx)
        out += win
        onEach(win)
        return DiagReport(System.currentTimeMillis(), summarize(ctx, facts), out)
    }

    /** Phần "Kết luận" gộp các dữ kiện. */
    fun summarize(ctx: Context, f: DiagFacts): CheckResult {
        fun s(id: Int, vararg a: Any) = ctx.getString(id, *a)
        val lines = ArrayList<String>()
        val usb = f.usb
        val dataLink = usb != null && usb.connected == true && (usb.configured == true || usb.dataFunctionActive)
        val chargeOnly = usb != null && usb.connected == true && !dataLink
        val noUsb = usb == null || usb.connected == false || !usb.broadcastSeen

        when {
            dataLink -> {
                val fn = usb!!.activeFunctions().joinToString().ifEmpty { "?" }
                lines += if (usb.fn("mtp") == true || usb.fn("ptp") == true) s(R.string.diag_sum_data_mtp) else s(R.string.diag_sum_data, fn)
            }
            chargeOnly -> lines += s(R.string.diag_sum_chargeonly)
            noUsb -> lines += s(R.string.diag_sum_nousb)
        }
        if (f.spacedeskInstalled) lines += s(R.string.diag_sum_spacedesk)
        if (dataLink && !f.tetherActive) lines += s(R.string.diag_sum_tether_hint)
        if (f.tetherActive) lines += s(R.string.diag_sum_tether)
        if (f.accessoryAttached) lines += s(R.string.diag_sum_aoa_ok)
        else if (dataLink) lines += s(R.string.diag_sum_aoa_wait)
        if (f.adbReverseOpen) lines += s(R.string.diag_sum_adb_ok)
        else if (f.adbEnabled == false) lines += s(R.string.diag_sum_adb_off)
        if (f.hostsFound > 0) lines += s(R.string.diag_sum_lan, f.hostsFound)
        if (f.chosenCodec != null) lines += s(R.string.diag_sum_codec, f.chosenCodec!!)

        val route = f.accessoryAttached || f.adbReverseOpen || f.tetherActive || f.hostsFound > 0
        val verdict = when {
            route -> Verdict.OK
            dataLink -> Verdict.WARN
            else -> Verdict.FAIL
        }
        val headline = when {
            route -> s(R.string.diag_sum_head_route)
            dataLink -> s(R.string.diag_sum_head_data_noroute)
            chargeOnly -> s(R.string.diag_sum_head_charge)
            else -> s(R.string.diag_sum_head_nousb)
        }
        val fix = when {
            route -> null
            dataLink -> s(R.string.diag_sum_fix_data)
            else -> s(R.string.diag_sum_fix_cable)
        }
        return CheckResult("summary", s(R.string.diag_summary_title), verdict, headline, lines, null, fix)
    }

    private fun mark(v: Verdict) = when (v) {
        Verdict.OK -> "[OK]"
        Verdict.WARN -> "[CANH BAO]"
        Verdict.FAIL -> "[LOI]"
        Verdict.INFO -> "[THONG TIN]"
    }

    fun buildText(ctx: Context, report: DiagReport): String {
        val ts = SimpleDateFormat("yyyy-MM-dd HH:mm:ss Z", Locale.US)
        val sb = StringBuilder()
        sb.append(ctx.getString(R.string.diag_report_header)).append('\n')
        sb.append(ctx.getString(R.string.diag_report_time, ts.format(Date(report.timeMillis)))).append("\n\n")
        fun block(r: CheckResult) {
            sb.append(mark(r.verdict)).append(' ').append(r.title).append(" - ").append(r.headline).append('\n')
            r.details.forEach { sb.append("    ").append(it).append('\n') }
            r.explanation?.let { sb.append("    ").append(ctx.getString(R.string.diag_why_label, it)).append('\n') }
            r.fix?.let { sb.append("    ").append(ctx.getString(R.string.diag_fix_label, it)).append('\n') }
            sb.append('\n')
        }
        block(report.summary)
        report.checks.forEach { block(it) }
        return sb.toString()
    }

    fun copy(ctx: Context, text: String) {
        val cm = ctx.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        cm.setPrimaryClip(ClipData.newPlainText("remoteview-diagnostics", text))
    }

    fun share(ctx: Context, text: String) {
        val send = Intent(Intent.ACTION_SEND).apply {
            type = "text/plain"
            putExtra(Intent.EXTRA_SUBJECT, ctx.getString(R.string.diag_report_header))
            putExtra(Intent.EXTRA_TEXT, text)
        }
        val chooser = Intent.createChooser(send, ctx.getString(R.string.diag_share)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        ctx.startActivity(chooser)
    }

    /** Lưu vào thư mục file riêng của app trên bộ nhớ ngoài; trả về đường dẫn hoặc null nếu lỗi. */
    fun save(ctx: Context, report: DiagReport, text: String): String? = try {
        val dir = ctx.getExternalFilesDir(null) ?: ctx.filesDir
        val name = "diagnostics-" + SimpleDateFormat("yyyyMMdd-HHmmss", Locale.US).format(Date(report.timeMillis)) + ".txt"
        val f = File(dir, name)
        f.writeText(text, Charsets.UTF_8)
        f.absolutePath
    } catch (_: Exception) {
        null
    }
}
