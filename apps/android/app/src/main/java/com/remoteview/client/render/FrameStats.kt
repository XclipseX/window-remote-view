package com.remoteview.client.render

import kotlin.math.ceil
import kotlin.math.sqrt

// UNBUILT: chưa biên dịch.

/** Số liệu khung hình đã tính (đơn vị: FPS, ms, ms^2). */
data class FrameStatsSnapshot(
    /** FPS trong 1 giây gần nhất. */
    val fpsNow: Float = 0f,
    /** FPS trung bình = 1000 / khoảng cách khung trung bình trong cửa sổ. */
    val avgFps: Float = 0f,
    /** 1% low = 1000 / trung bình của 1% khoảng cách khung tệ nhất. */
    val low1Fps: Float = 0f,
    val meanMs: Float = 0f,
    val stdMs: Float = 0f,
    /** Phương sai thời gian khung (ms^2). */
    val varianceMs2: Float = 0f,
    /** Số khung trong cửa sổ. */
    val frames: Int = 0,
)

/**
 * Bộ đo thời điểm trình chiếu khung hình, cửa sổ trượt [windowNs] (mặc định 10 giây).
 * Thuần Kotlin, an toàn đa luồng (record gọi từ thread codec/main, snapshot từ main).
 * Khoảng cách > [IDLE_GAP_NS] (hình đứng yên) bị loại khỏi thống kê khoảng cách để không làm méo 1% low/variance.
 */
class FrameStats(
    private val windowNs: Long = 10_000_000_000L,
    capacity: Int = 4096,
) {
    private val buf = LongArray(capacity)
    private var head = 0 // vị trí ghi kế tiếp
    private var size = 0
    private val lock = Any()

    /** Ghi thời điểm (ns, cùng gốc System.nanoTime) một khung đã được trả ra surface. */
    fun record(tsNs: Long) {
        synchronized(lock) {
            // Thời gian đi lùi (hiếm): coi như bắt đầu lại.
            if (size > 0) {
                val last = buf[(head - 1 + buf.size) % buf.size]
                if (tsNs < last) { size = 0; head = 0 }
            }
            buf[head] = tsNs
            head = (head + 1) % buf.size
            if (size < buf.size) size++
        }
    }

    fun reset() {
        synchronized(lock) { size = 0; head = 0 }
    }

    fun snapshot(nowNs: Long): FrameStatsSnapshot {
        val ts: LongArray
        synchronized(lock) {
            val tmp = LongArray(size)
            var n = 0
            val start = (head - size + buf.size) % buf.size
            for (i in 0 until size) {
                val t = buf[(start + i) % buf.size]
                if (nowNs - t <= windowNs) tmp[n++] = t
            }
            ts = tmp.copyOf(n)
        }
        if (ts.isEmpty()) return FrameStatsSnapshot()
        var inLastSec = 0
        for (t in ts) if (nowNs - t <= ONE_SEC_NS) inLastSec++
        val fpsNow = inLastSec.toFloat()
        if (ts.size < 2) return FrameStatsSnapshot(fpsNow = fpsNow, frames = ts.size)

        val iv = DoubleArray(ts.size - 1)
        var m = 0
        for (i in 1 until ts.size) {
            val d = ts[i] - ts[i - 1]
            if (d in 1..IDLE_GAP_NS) iv[m++] = d / 1_000_000.0
        }
        if (m == 0) return FrameStatsSnapshot(fpsNow = fpsNow, frames = ts.size)
        var sum = 0.0
        for (i in 0 until m) sum += iv[i]
        val mean = sum / m
        var sq = 0.0
        for (i in 0 until m) { val e = iv[i] - mean; sq += e * e }
        val variance = sq / m
        val sorted = iv.copyOf(m)
        sorted.sort() // tăng dần
        val worstCount = maxOf(1, ceil(m * 0.01).toInt())
        var worstSum = 0.0
        for (i in 0 until worstCount) worstSum += sorted[m - 1 - i]
        val worstMean = worstSum / worstCount
        return FrameStatsSnapshot(
            fpsNow = fpsNow,
            avgFps = (1000.0 / mean).toFloat(),
            low1Fps = (1000.0 / worstMean).toFloat(),
            meanMs = mean.toFloat(),
            stdMs = sqrt(variance).toFloat(),
            varianceMs2 = variance.toFloat(),
            frames = ts.size,
        )
    }

    companion object {
        const val ONE_SEC_NS = 1_000_000_000L
        const val IDLE_GAP_NS = 500_000_000L
    }
}
