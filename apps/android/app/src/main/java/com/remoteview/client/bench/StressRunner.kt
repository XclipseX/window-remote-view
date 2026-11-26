package com.remoteview.client.bench

import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.hardware.display.DisplayManager
import android.os.BatteryManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import android.view.Display
import java.util.Locale
import kotlin.math.abs

// UNBUILT: chưa biên dịch.

class SystemProbe(private val ctx: Context) {
    class Reading(
        val headroom: Float, val thermal: Int, val pct: Int, val tempC: Float,
        val currentMa: Float, val voltageMv: Int, val watts: Float, val refreshHz: Float, val plugged: Boolean,
    )

    private val pm = ctx.getSystemService(Context.POWER_SERVICE) as PowerManager
    private val bm = ctx.getSystemService(Context.BATTERY_SERVICE) as BatteryManager
    private val dm = ctx.getSystemService(Context.DISPLAY_SERVICE) as DisplayManager

    fun read(): Reading {
        val headroom = if (Build.VERSION.SDK_INT >= 30) runCatching { pm.getThermalHeadroom(10) }.getOrDefault(Float.NaN) else Float.NaN
        val thermal = if (Build.VERSION.SDK_INT >= 29) runCatching { pm.currentThermalStatus }.getOrDefault(-1) else -1
        val pct = runCatching { bm.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY) }.getOrDefault(-1)
        val raw = runCatching { bm.getLongProperty(BatteryManager.BATTERY_PROPERTY_CURRENT_NOW) }.getOrDefault(Long.MIN_VALUE)
        var ua = if (raw == Long.MIN_VALUE || raw == 0L) Double.NaN else abs(raw.toDouble())
        // Một số máy báo mA thay vì µA: giá trị < 20000 coi như mA.
        if (!ua.isNaN() && ua < 20_000) ua *= 1000.0
        val bat: Intent? = runCatching { ctx.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED)) }.getOrNull()
        val mv = bat?.getIntExtra(BatteryManager.EXTRA_VOLTAGE, -1) ?: -1
        val temp = (bat?.getIntExtra(BatteryManager.EXTRA_TEMPERATURE, Int.MIN_VALUE) ?: Int.MIN_VALUE)
            .let { if (it == Int.MIN_VALUE) Float.NaN else it / 10f }
        val plugged = (bat?.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0) ?: 0) != 0
        val watts = if (!ua.isNaN() && mv > 0) (ua * mv / 1e9).toFloat() else Float.NaN
        val hz = runCatching { dm.getDisplay(Display.DEFAULT_DISPLAY).refreshRate }.getOrDefault(Float.NaN)
        return Reading(headroom, thermal, pct, temp, if (ua.isNaN()) Float.NaN else (ua / 1000.0).toFloat(), mv, watts, hz, plugged)
    }
}

class StressRunner(
    private val ctx: Context,
    private val cfg: BenchConfig,
    private val listener: Listener,
) {
    interface Listener {
        fun onPhase(msg: String)
        fun onSample(samples: List<BenchSample>)
        fun onFinished(summary: BenchSummary)
    }

    private val main = Handler(Looper.getMainLooper())
    @Volatile private var stopRequested = false
    private var thread: Thread? = null

    fun start() {
        thread = Thread({ runBench() }, "bench-main").also { it.start() }
    }

    fun stop() { stopRequested = true }

    private fun phase(s: String) = main.post { listener.onPhase(s) }

    private fun runBench() {
        val samples = ArrayList<BenchSample>()
        var all = LongArray(1 shl 16)
        var allN = 0
        val workers = ArrayList<DecodeWorker>()
        var stopReason = "Hoàn tất theo thời lượng đã chọn"
        var clip: Clip? = null
        try {
            phase("Đang tạo clip thử bằng bộ mã hóa phần cứng...")
            clip = try {
                ClipGenerator.get(cfg.codec.mime, cfg.width, cfg.height, cfg.fps, cfg.bitrateMbps,
                    { d, t -> phase("Đang mã hóa clip: $d/$t khung") }, { stopRequested })
            } catch (e: ClipException) {
                finish(failure(e.message ?: "Lỗi tạo clip")); return
            }
            phase("Đang khởi động ${cfg.instances} bộ giải mã...")
            for (i in 0 until cfg.instances) {
                val w = DecodeWorker(i, ctx, cfg, clip)
                workers.add(w)
                if (!w.start()) {
                    finish(failure(w.error ?: "Không khởi động được bộ giải mã")); return
                }
            }
            phase("Đang chạy: ${cfg.decoderName} (${workers[0].tierLabel})")
            val probe = SystemProbe(ctx)
            val startMs = SystemClock.elapsedRealtime()
            var lastMs = startMs
            var lastOut = 0L
            var lastMissed = 0L
            var next = startMs + 1000
            val limit = if (cfg.allowCritical) 4 else 3 // SEVERE=3, CRITICAL=4
            while (!stopRequested) {
                val sleep = next - SystemClock.elapsedRealtime()
                if (sleep > 0) Thread.sleep(sleep)
                next += 1000
                val now = SystemClock.elapsedRealtime()
                val dt = (now - lastMs) / 1000.0
                lastMs = now
                val out = workers.sumOf { it.outCount.get() }
                val miss = workers.sumOf { it.missed.get() }
                val lat = ArrayList<Long>()
                var adpfSum = 0f; var adpfCnt = 0
                for (w in workers) {
                    for (v in w.latencies.drain()) lat.add(v)
                    val a = w.drainAdpfAvgMs()
                    if (!a.isNaN()) { adpfSum += a; adpfCnt++ }
                }
                val arr = lat.toLongArray().also { it.sort() }
                if (allN + arr.size > all.size) all = all.copyOf(maxOf(all.size * 2, allN + arr.size))
                System.arraycopy(arr, 0, all, allN, arr.size); allN += arr.size
                val p = probe.read()
                val s = BenchSample(
                    tSec = ((now - startMs) / 1000).toInt(),
                    fps = ((out - lastOut) / dt).toFloat(),
                    p50Ms = pct(arr, 0.50) / 1e6f, p99Ms = pct(arr, 0.99) / 1e6f,
                    maxMs = if (arr.isEmpty()) Float.NaN else arr.last() / 1e6f,
                    missed = miss - lastMissed,
                    headroom = p.headroom, thermalStatus = p.thermal, batteryPct = p.pct, batteryTempC = p.tempC,
                    currentMa = p.currentMa, voltageMv = p.voltageMv, watts = p.watts, refreshHz = p.refreshHz,
                    adpfMs = if (adpfCnt == 0) Float.NaN else adpfSum / adpfCnt, plugged = p.plugged,
                )
                lastOut = out; lastMissed = miss
                samples.add(s)
                val copy = ArrayList(samples)
                main.post { listener.onSample(copy) }
                if (p.thermal >= limit) {
                    stopReason = "Dừng an toàn: trạng thái nhiệt ${thermalName(p.thermal)} (>= ${thermalName(limit)})"
                    break
                }
                val bad = workers.firstOrNull { it.error != null }
                if (bad != null) { stopReason = "Lỗi giải mã: ${bad.error}"; break }
                if ((now - startMs) / 1000 >= cfg.durationSec) break
            }
            if (stopRequested && stopReason.startsWith("Hoàn tất")) stopReason = "Người dùng dừng"
        } catch (e: Exception) {
            stopReason = "Lỗi: $e"
        } finally {
            workers.forEach { runCatching { it.stop() } }
        }
        val c = clip
        finish(buildSummary(cfg, c, workers.firstOrNull()?.tierLabel ?: "", samples, all.copyOf(allN).also { it.sort() }, stopReason))
    }

    private fun failure(msg: String) = BenchSummary("Không chạy được: $msg", "", null)

    private fun finish(s: BenchSummary) { main.post { listener.onFinished(s) } }

    companion object {
        fun thermalName(s: Int) = when (s) {
            0 -> "NONE"; 1 -> "LIGHT"; 2 -> "MODERATE"; 3 -> "SEVERE"; 4 -> "CRITICAL"; 5 -> "EMERGENCY"; 6 -> "SHUTDOWN"
            else -> "?"
        }

        fun pct(sorted: LongArray, q: Double): Float {
            if (sorted.isEmpty()) return Float.NaN
            val i = ((sorted.size - 1) * q).toInt().coerceIn(0, sorted.size - 1)
            return sorted[i].toFloat()
        }

        private fun mean(l: List<Float>): Float {
            val v = l.filter { !it.isNaN() }
            return if (v.isEmpty()) Float.NaN else v.average().toFloat()
        }

        private fun f(v: Float, d: Int = 1) = if (v.isNaN()) "n/a" else String.format(Locale.US, "%.${d}f", v)

        fun buildSummary(
            cfg: BenchConfig, clip: Clip?, tier: String, s: List<BenchSample>, allLat: LongArray, reason: String,
        ): BenchSummary {
            val sb = StringBuilder()
            sb.appendLine("=== Kiểm tra giới hạn giải mã ===")
            sb.appendLine("Máy: ${Build.MANUFACTURER} ${Build.MODEL} (Android ${Build.VERSION.RELEASE}, API ${Build.VERSION.SDK_INT})")
            if (Build.VERSION.SDK_INT >= 31) sb.appendLine("SoC: ${Build.SOC_MANUFACTURER} ${Build.SOC_MODEL}")
            sb.appendLine("Chế độ: ${cfg.mode}  Codec: ${cfg.codec.label}  Decoder: ${cfg.decoderName} [$tier]")
            sb.appendLine("Độ phân giải: ${cfg.width}x${cfg.height}  FPS đích: ${cfg.fps}  Số decoder song song: ${cfg.instances}  ADPF: ${if (cfg.adpf) "bật" else "tắt"}")
            if (clip != null) sb.appendLine("Clip: encoder ${clip.encoderName}, bitrate thực ${f(clip.actualMbps.toFloat())} Mbps (yêu cầu ${cfg.bitrateMbps}), ${clip.frames.size} khung")
            sb.appendLine("Kết thúc: $reason  (${s.size} s đo)")
            var throttled: Boolean? = null
            if (s.size >= 3) {
                val warm = if (s.size > 20) 2 else 0
                val win = minOf(15, maxOf(1, s.size / 3))
                val first = s.drop(warm).take(win)
                val last = s.takeLast(win)
                val fpsF = mean(first.map { it.fps }); val fpsL = mean(last.map { it.fps })
                val p99F = mean(first.map { it.p99Ms }); val p99L = mean(last.map { it.p99Ms })
                val peakFps = s.maxOf { it.fps }
                val drop = if (fpsF > 0) (fpsF - fpsL) / fpsF else 0f
                val rise = if (!p99F.isNaN() && p99F > 0 && !p99L.isNaN()) (p99L - p99F) / p99F else 0f
                val maxHr = last.maxOf { if (it.headroom.isNaN()) 0f else it.headroom }
                val maxSt = last.maxOf { it.thermalStatus }
                val thermalSignal = maxHr >= 0.9f || maxSt >= 2
                val degraded = drop > 0.10f || rise > 0.30f
                throttled = degraded && thermalSignal
                sb.appendLine("--- Số liệu (cửa sổ $win s đầu so với $win s cuối) ---")
                sb.appendLine("FPS: đỉnh ${f(peakFps)}, đầu ${f(fpsF)}, cuối ${f(fpsL)} (đổi ${f(-drop * 100)}%)")
                sb.appendLine("p99 trễ (ms): đầu ${f(p99F, 2)}, cuối ${f(p99L, 2)} (đổi ${f(rise * 100)}%)")
                sb.appendLine("Nhiệt cuối: headroom tối đa ${f(maxHr, 2)}, trạng thái tối đa ${thermalName(maxSt)}; đỉnh cả phiên: headroom ${f(s.maxOf { if (it.headroom.isNaN()) 0f else it.headroom }, 2)}, ${thermalName(s.maxOf { it.thermalStatus })}")
                val wAvg = mean(s.map { it.watts })
                sb.appendLine("Công suất ước tính TB: ${f(wAvg, 2)} W${if (s.any { it.plugged }) " (đang cắm sạc: KHÔNG chính xác)" else ""}; pin ${s.first().batteryPct}% -> ${s.last().batteryPct}%")
                sb.appendLine(
                    when {
                        throttled -> "KẾT LUẬN: bị bóp xung (hiệu năng giảm cùng tín hiệu nhiệt)."
                        degraded -> "KẾT LUẬN: hiệu năng giảm nhưng KHÔNG có tín hiệu nhiệt rõ ràng (có thể DVFS/chính sách hệ thống/nguồn)."
                        else -> "KẾT LUẬN: ổn định, không bị bóp xung trong phạm vi thử."
                    },
                )
            } else {
                sb.appendLine("Quá ít mẫu để kết luận.")
            }
            if (allLat.isNotEmpty()) {
                sb.appendLine("Độ trễ toàn phiên (ms): p50 ${f(pct(allLat, .5) / 1e6f, 2)}  p95 ${f(pct(allLat, .95) / 1e6f, 2)}  p99 ${f(pct(allLat, .99) / 1e6f, 2)}  max ${f(allLat.last() / 1e6f, 2)}")
                if (cfg.mode == BenchMode.THROUGHPUT) sb.appendLine("(Chế độ throughput: độ trễ gồm cả thời gian xếp hàng trong decoder, chỉ để tham khảo.)")
            }
            sb.appendLine("Tick bị bỏ vì decoder không còn input buffer: ${s.sumOf { it.missed }}")

            val csv = StringBuilder("t_s,fps,p50_ms,p99_ms,max_ms,missed,headroom,thermal_status,battery_pct,battery_temp_c,current_ma,voltage_mv,watts,refresh_hz,adpf_ms,plugged\n")
            for (x in s) {
                csv.append("${x.tSec},${f(x.fps)},${f(x.p50Ms, 3)},${f(x.p99Ms, 3)},${f(x.maxMs, 3)},${x.missed},${f(x.headroom, 3)},${x.thermalStatus},${x.batteryPct},${f(x.batteryTempC)},${f(x.currentMa, 0)},${x.voltageMv},${f(x.watts, 3)},${f(x.refreshHz)},${f(x.adpfMs, 3)},${x.plugged}\n")
            }
            return BenchSummary(sb.toString(), csv.toString(), throttled)
        }
    }
}
