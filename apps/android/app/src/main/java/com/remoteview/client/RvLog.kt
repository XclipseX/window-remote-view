package com.remoteview.client

import android.util.Log
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
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
    private const val MAX_LINES = 500

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
            buf.addLast("${fmt.format(Date())} $level $full")
            while (buf.size > MAX_LINES) buf.removeFirst()
        }
        _version.value = _version.value + 1
    }

    fun lines(): List<String> = synchronized(lock) { buf.toList() }

    fun text(): String = lines().joinToString("\n")

    fun clear() {
        synchronized(lock) { buf.clear() }
        _version.value = _version.value + 1
    }
}
