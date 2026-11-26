package com.remoteview.client.bench

import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.view.WindowManager
import android.widget.Toast
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.remoteview.client.R
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

// UNBUILT: chưa biên dịch.

private fun Context.findActivity(): Activity? {
    var c: Context? = this
    while (c is ContextWrapper) { if (c is Activity) return c; c = c.baseContext }
    return null
}

private fun saveReport(ctx: Context, s: BenchSummary): File? = runCatching {
    val dir = File(ctx.getExternalFilesDir(null), "bench").also { it.mkdirs() }
    val stamp = SimpleDateFormat("yyyyMMdd_HHmmss", Locale.US).format(Date())
    File(dir, "stress_$stamp.txt").writeText(s.text)
    File(dir, "stress_$stamp.csv").also { it.writeText(s.csv) }
}.getOrNull()

@Composable
private fun <T> Picker(label: String, value: String, options: List<T>, optionLabel: (T) -> String, enabled: Boolean = true, onPick: (T) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Column {
        Text(label, style = MaterialTheme.typography.labelMedium)
        Box {
            OutlinedButton(onClick = { open = true }, enabled = enabled, modifier = Modifier.fillMaxWidth()) {
                Text(value, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                options.forEach { o ->
                    DropdownMenuItem(text = { Text(optionLabel(o)) }, onClick = { open = false; onPick(o) })
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun BenchScreen(onBack: () -> Unit) {
    val ctx = LocalContext.current
    val nativeRes = remember { BenchDevice.nativeResolution(ctx) }
    val resolutions = remember {
        listOf(nativeRes, BenchResolution(2800, 1752), BenchResolution(2560, 1440), BenchResolution(1920, 1080))
    }

    var mode by remember { mutableStateOf(BenchMode.PACED) }
    var codec by remember { mutableStateOf(BenchCodec.HEVC) }
    val decoders = remember(codec) { BenchDevice.hardwareDecoders(codec.mime) }
    var decoder by remember(codec) { mutableStateOf(decoders.firstOrNull { !it.contains("lowlatency") } ?: decoders.firstOrNull() ?: "") }
    var res by remember { mutableStateOf(nativeRes) }
    var fps by remember { mutableStateOf(120) }
    var durationSec by remember { mutableStateOf(30) }
    var instances by remember { mutableStateOf(1) }
    var bitrate by remember { mutableStateOf(80) }
    var adpf by remember { mutableStateOf(true) }
    var allowCritical by remember { mutableStateOf(false) }

    var running by remember { mutableStateOf(false) }
    var phase by remember { mutableStateOf("") }
    var samples by remember { mutableStateOf(emptyList<BenchSample>()) }
    var summary by remember { mutableStateOf<BenchSummary?>(null) }
    val samplesState: (List<BenchSample>) -> Unit = { samples = it }
    val summaryState: (BenchSummary) -> Unit = { summary = it }
    var savedPath by remember { mutableStateOf<String?>(null) }
    val runner = remember { arrayOfNulls<StressRunner>(1) }

    DisposableEffect(Unit) { onDispose { runner[0]?.stop() } }
    DisposableEffect(running) {
        if (running) ctx.findActivity()?.window?.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        onDispose { }
    }

    val effFps = if (mode == BenchMode.PACED) fps else 120
    val durations = if (mode == BenchMode.SUSTAINED) listOf(60, 120) else listOf(15, 30, 60, 120)
    val minStr = { n: Int -> ctx.getString(R.string.bench_min, n) }
    val secStr = { n: Int -> ctx.getString(R.string.bench_sec, n) }
    val durLabel = { d: Int -> if (d >= 120 && d % 60 == 0) minStr(d / 60) else if (d == 60) minStr(1) else secStr(d) }
    val sizeOk = decoder.isEmpty() || BenchDevice.supportsSize(decoder, codec.mime, res.width, res.height, effFps)

    fun start() {
        val cfg = BenchConfig(mode, codec, decoder, res.width, res.height, effFps, durationSec, instances, bitrate, adpf, allowCritical)
        samples = emptyList(); summary = null; savedPath = null; running = true; phase = ""
        val r = StressRunner(ctx.applicationContext, cfg, object : StressRunner.Listener {
            override fun onPhase(msg: String) { phase = msg }
            override fun onSample(samples: List<BenchSample>) { samplesState(samples) }
            override fun onFinished(summary: BenchSummary) {
                running = false
                summaryState(summary)
                savedPath = saveReport(ctx, summary)?.absolutePath
            }
        })
        runner[0] = r
        r.start()
    }

    val last = samples.lastOrNull()
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.bench_title)) },
                navigationIcon = { TextButton(onClick = onBack) { Text(stringResource(R.string.bench_back)) } },
            )
        },
    ) { inner ->
        Column(
            Modifier.fillMaxSize().padding(inner).padding(horizontal = 16.dp).verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(stringResource(R.string.bench_unplug), style = MaterialTheme.typography.bodySmall)
            if (last?.plugged == true) {
                Text(stringResource(R.string.bench_plugged_warn), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
            }
            val modeLabel = { m: BenchMode ->
                ctx.getString(when (m) {
                    BenchMode.THROUGHPUT -> R.string.bench_mode_throughput
                    BenchMode.PACED -> R.string.bench_mode_paced
                    BenchMode.SUSTAINED -> R.string.bench_mode_sustained
                })
            }
            Picker(stringResource(R.string.bench_mode), modeLabel(mode), BenchMode.values().toList(), modeLabel, !running) {
                mode = it
                durationSec = if (it == BenchMode.SUSTAINED) 60 else 30
            }
            Picker(stringResource(R.string.bench_codec), codec.label, BenchCodec.values().toList(), { it.label }, !running) { codec = it }
            Picker(
                stringResource(R.string.bench_decoder),
                decoder.ifEmpty { stringResource(R.string.bench_no_decoder) },
                decoders, { it }, !running && decoders.isNotEmpty(),
            ) { decoder = it }
            Picker(stringResource(R.string.bench_resolution), res.label, resolutions, { it.label }, !running) { res = it }
            if (!sizeOk) Text(stringResource(R.string.bench_size_warn), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
            if (mode == BenchMode.PACED) {
                Picker(stringResource(R.string.bench_fps), "$fps", listOf(60, 120, 144), { "$it" }, !running) { fps = it }
            } else {
                Text(stringResource(R.string.bench_fps_fixed, effFps), style = MaterialTheme.typography.bodySmall)
            }
            Picker(stringResource(R.string.bench_duration), durLabel(durationSec), durations, durLabel, !running) { durationSec = it }
            Picker(stringResource(R.string.bench_instances), if (instances == 0) "Tối đa" else "$instances", listOf(1, 2, 3, 4, 6, 8, 12, 16, 0), { if (it == 0) "Tối đa" else "$it" }, !running) { instances = it }
            Picker(stringResource(R.string.bench_bitrate), "$bitrate", listOf(20, 40, 80, 120), { "$it" }, !running) { bitrate = it }
            Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                Text(stringResource(R.string.bench_adpf), Modifier.weight(1f))
                Switch(checked = adpf, onCheckedChange = { adpf = it }, enabled = !running)
            }
            Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                Text(stringResource(R.string.bench_allow_critical), Modifier.weight(1f))
                Switch(checked = allowCritical, onCheckedChange = { allowCritical = it }, enabled = !running)
            }

            if (!running) {
                Button(onClick = { start() }, enabled = decoder.isNotEmpty(), modifier = Modifier.fillMaxWidth()) {
                    Text(stringResource(R.string.bench_start))
                }
            } else {
                Button(onClick = { runner[0]?.stop() }, modifier = Modifier.fillMaxWidth()) { Text(stringResource(R.string.bench_stop)) }
                Text(phase, style = MaterialTheme.typography.bodySmall)
                LinearProgressIndicator(
                    progress = { ((last?.tSec ?: 0).toFloat() / durationSec).coerceIn(0f, 1f) },
                    modifier = Modifier.fillMaxWidth(),
                )
            }

            if (last != null) {
                Text(
                    stringResource(
                        R.string.bench_live,
                        StressRunner.thermalName(last.thermalStatus), last.batteryPct,
                        if (last.batteryTempC.isNaN()) "n/a" else String.format(Locale.US, "%.1f", last.batteryTempC),
                        if (last.refreshHz.isNaN()) "n/a" else String.format(Locale.US, "%.0f", last.refreshHz),
                        if (adpf) "on" else "off",
                    ),
                    style = MaterialTheme.typography.bodySmall,
                )
                val c1 = MaterialTheme.colorScheme.primary
                val c2 = MaterialTheme.colorScheme.tertiary
                val c3 = MaterialTheme.colorScheme.error
                LineChart(stringResource(R.string.bench_chart_fps), "fps", listOf(ChartSeries("fps", samples.map { it.fps }, c1)), yMin = 0f)
                LineChart(
                    stringResource(R.string.bench_chart_latency), "ms",
                    listOf(ChartSeries("p50", samples.map { it.p50Ms }, c1), ChartSeries("p99", samples.map { it.p99Ms }, c3)), yMin = 0f,
                )
                LineChart(stringResource(R.string.bench_chart_headroom), "", listOf(ChartSeries("headroom", samples.map { it.headroom }, c3), ChartSeries("status/10", samples.map { if (it.thermalStatus < 0) Float.NaN else it.thermalStatus / 10f }, c2)), yMin = 0f)
                LineChart(stringResource(R.string.bench_chart_power), "W", listOf(ChartSeries("W", samples.map { it.watts }, c2)), yMin = 0f)
            }

            summary?.let { sm ->
                Text(sm.text, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace)
                savedPath?.let { Text(stringResource(R.string.bench_saved, it), style = MaterialTheme.typography.bodySmall) }
                val full = sm.text + "\n" + sm.csv
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedButton(onClick = {
                        val cm = ctx.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                        cm.setPrimaryClip(ClipData.newPlainText("bench", full))
                        Toast.makeText(ctx, ctx.getString(R.string.bench_copied), Toast.LENGTH_SHORT).show()
                    }) { Text(stringResource(R.string.bench_copy)) }
                    OutlinedButton(onClick = {
                        val i = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, sm.text)
                        ctx.startActivity(Intent.createChooser(i, null).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                    }) { Text(stringResource(R.string.bench_share)) }
                }
            }
            Box(Modifier.padding(bottom = 24.dp))
        }
    }
}
