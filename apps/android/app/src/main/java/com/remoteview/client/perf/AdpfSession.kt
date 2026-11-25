package com.remoteview.client.perf

import android.content.Context
import android.os.Build
import android.os.PerformanceHintManager
import android.util.Log
import java.io.Closeable

// UNBUILT: chưa biên dịch.

/**
 * ADPF PerformanceHintManager session (API 31+). Các thread truyền vào nên là thread giải mã/trình chiếu
 * (vd Process.myTid() của HandlerThread codec). Không làm gì trên API < 31.
 */
class AdpfSession(context: Context, tids: IntArray, fps: Int) : Closeable {

    private var session: PerformanceHintManager.Session? = null

    init {
        if (Build.VERSION.SDK_INT >= 31) {
            try {
                val mgr = context.getSystemService(PerformanceHintManager::class.java)
                session = mgr?.createHintSession(tids, targetNs(fps))
            } catch (e: Exception) {
                Log.w(TAG, "Không tạo được hint session: $e")
            }
        }
    }

    val isActive: Boolean get() = session != null

    /** Báo thời gian xử lý thực tế của một khung (giải mã -> trình chiếu). */
    fun report(actualWorkNs: Long) {
        try { session?.reportActualWorkDuration(actualWorkNs) } catch (_: Exception) {}
    }

    fun updateTarget(fps: Int) {
        try { session?.updateTargetWorkDuration(targetNs(fps)) } catch (_: Exception) {}
    }

    override fun close() {
        try { session?.close() } catch (_: Exception) {}
        session = null
    }

    companion object {
        private const val TAG = "RvAdpf"

        /** 6.5 ms @120, 12 ms @60 (research-2); 90 và 30 nội suy theo chu kỳ khung. */
        fun targetNs(fps: Int): Long = when {
            fps >= 120 -> 6_500_000L
            fps >= 90 -> 9_000_000L
            fps >= 60 -> 12_000_000L
            else -> 25_000_000L
        }
    }
}
