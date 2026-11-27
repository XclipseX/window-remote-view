package com.remoteview.client

import android.content.Context
import android.os.Build
import android.util.Log
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import java.io.BufferedWriter
import java.io.File
import java.io.FileOutputStream
import java.io.OutputStreamWriter
import java.text.SimpleDateFormat
import java.util.ArrayDeque
import java.util.Date
import java.util.Locale

/**
 * Nhật ký của chính app: ghi ra logcat (tag "RemoteView") và giữ một vòng đệm [MAX_LINES] dòng gần nhất
 * để màn hình "Nhật ký kết nối" hiển thị / sao chép.
 */
object RvLog {
    const val TAG = "RemoteView"
    private const val MAX_LINES = 5000
    private const val KEEP_FILES = 10

    private val lock = Any()
    private val buf = ArrayDeque<String>(MAX_LINES + 1)
    private val fmt = SimpleDateFormat("HH:mm:ss.SSS", Locale.US)
    private val _version = MutableStateFlow(0)

    /** Tăng mỗi khi có dòng mới; UI collect để biết cần đọc lại [lines]. */
    val version: StateFlow<Int> = _version

    fun i(msg: String) = add('I', msg, null)
    fun w(msg: String, t: Throwable? = null) = add('W', msg, t)
    fun e(msg: String, t: Throwable? = null) = add('E', msg, t)

    private fun add(level: Char, msg: String, t: Throwable?) {
        val full = if (t != null) "$msg (${t.javaClass.simpleName}: ${t.message})" else msg
        when (level) {
            'W' -> Log.w(TAG, full)
            'E' -> Log.e(TAG, full)
            else -> Log.i(TAG, full)
        }
        synchronized(lock) {
            val line = "${fmt.format(Date())} $level $full"
            buf.addLast(line)
            while (buf.size > MAX_LINES) buf.removeFirst()
            writeFile(line, flushNow = level == 'E')
        }
        _version.value = _version.value + 1
    }

    fun lines(): List<String> = synchronized(lock) { buf.toList() }

    fun text(): String = lines().joinToString("\n")

    fun clear() {
        synchronized(lock) { buf.clear() }
        _version.value = _version.value + 1
    }

    // ---- Ghi file: filesDir/logs/app-<ngày-giờ>.log, giữ KEEP_FILES file gần nhất ----

    private var writer: BufferedWriter? = null
    private var currentFile: File? = null
    private var logsDir: File? = null

    /** Gọi trong Application.onCreate: mở file log mới, ghi tiêu đề, và đẩy các dòng đã có trong bộ nhớ. */
    fun init(ctx: Context) {
        synchronized(lock) {
            if (writer != null) return
            try {
                val dir = File(ctx.applicationContext.filesDir, "logs").also { it.mkdirs() }
                logsDir = dir
                val old = dir.listFiles { f -> f.name.startsWith("app-") && f.name.endsWith(".log") }
                    ?.sortedBy { it.name } ?: emptyList()
                for (f in old.take((old.size - (KEEP_FILES - 1)).coerceAtLeast(0))) f.delete()
                val name = "app-" + SimpleDateFormat("yyyyMMdd-HHmmss", Locale.US).format(Date()) + ".log"
                val f = File(dir, name)
                currentFile = f
                writer = BufferedWriter(OutputStreamWriter(FileOutputStream(f, true), Charsets.UTF_8), 16 * 1024)
                val pending = buf.toList()
                writeHeader(ctx)
                for (l in pending) writeFile(l, false)
                writer?.flush()
            } catch (t: Throwable) {
                Log.w(TAG, "không mở được file log: ${t.message}")
                writer = null
            }
        }
        val t = Thread({
            while (true) {
                try { Thread.sleep(1000) } catch (_: InterruptedException) { return@Thread }
                synchronized(lock) { try { writer?.flush() } catch (_: Throwable) {} }
            }
        }, "rv-log-flush")
        t.isDaemon = true
        t.start()
    }

    private fun writeHeader(ctx: Context) {
        val vn = try { ctx.packageManager.getPackageInfo(ctx.packageName, 0).versionName } catch (_: Throwable) { "?" }
        val soc = if (Build.VERSION.SDK_INT >= 31) "${Build.SOC_MANUFACTURER} ${Build.SOC_MODEL}" else Build.HARDWARE
        val head = listOf(
            "=== Remote View log ===",
            "Phiên bản app: $vn",
            "GIT_SHA: ${BuildConfig.GIT_SHA}",
            "Thiết bị: ${Build.MANUFACTURER} ${Build.MODEL} (${Build.DEVICE})",
            "Android: ${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT})",
            "SoC: $soc (${Build.BOARD}/${Build.HARDWARE})",
            "Bắt đầu: " + SimpleDateFormat("yyyy-MM-dd HH:mm:ss", Locale.US).format(Date()),
            "=======================",
        )
        for (h in head) writeFile(h, false)
    }

    private fun writeFile(line: String, flushNow: Boolean) {
        val w = writer ?: return
        try {
            w.write(line)
            w.write("\n")
            if (flushNow) w.flush()
        } catch (_: Throwable) {
        }
    }

    /** Ghi hết bộ đệm xuống đĩa (trước khi chia sẻ / gửi / khi crash). */
    fun flush() {
        synchronized(lock) { try { writer?.flush() } catch (_: Throwable) {} }
    }

    /** Tất cả file log (cũ -> mới), kể cả file hiện tại. */
    fun logFiles(): List<File> {
        flush()
        return logsDir?.listFiles { f -> f.name.startsWith("app-") && f.name.endsWith(".log") }
            ?.sortedBy { it.name } ?: emptyList()
    }

    /** Nội dung file log hiện tại (UTF-8), tối đa [maxBytes] byte cuối; rỗng nếu chưa có file. */
    fun currentFileBytes(maxBytes: Int = 2 * 1024 * 1024): ByteArray {
        flush()
        val f = currentFile ?: return text().toByteArray(Charsets.UTF_8)
        return try {
            val len = f.length()
            if (len <= maxBytes) f.readBytes()
            else java.io.RandomAccessFile(f, "r").use { r ->
                r.seek(len - maxBytes)
                ByteArray(maxBytes).also { r.readFully(it) }
            }
        } catch (_: Throwable) { text().toByteArray(Charsets.UTF_8) }
    }
}
