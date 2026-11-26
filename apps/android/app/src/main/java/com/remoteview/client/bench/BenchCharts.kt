package com.remoteview.client.bench

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.dp
import java.util.Locale

// UNBUILT: chưa biên dịch.

class ChartSeries(val name: String, val values: List<Float>, val color: Color)

@Composable
fun LineChart(title: String, unit: String, series: List<ChartSeries>, modifier: Modifier = Modifier, yMin: Float? = null) {
    val grid = MaterialTheme.colorScheme.outlineVariant
    val finite = series.flatMap { it.values }.filter { !it.isNaN() && !it.isInfinite() }
    val lo = yMin ?: (finite.minOrNull() ?: 0f)
    var hi = finite.maxOrNull() ?: 1f
    if (hi - lo < 1e-3f) hi = lo + 1f
    val legend = series.joinToString("   ") { s ->
        val last = s.values.lastOrNull { !it.isNaN() }
        "${s.name}: ${if (last == null) "n/a" else String.format(Locale.US, "%.2f", last)}"
    }
    Column(modifier.fillMaxWidth()) {
        Text("$title ($unit)  [${String.format(Locale.US, "%.1f", lo)} .. ${String.format(Locale.US, "%.1f", hi)}]", style = MaterialTheme.typography.labelLarge)
        Text(legend, style = MaterialTheme.typography.bodySmall)
        Canvas(Modifier.fillMaxWidth().height(100.dp)) {
            for (i in 0..2) {
                val y = size.height * i / 2f
                drawLine(grid, Offset(0f, y), Offset(size.width, y), strokeWidth = 1f)
            }
            for (s in series) {
                val n = s.values.size
                if (n < 2) continue
                val path = Path()
                var pen = false
                for ((i, v) in s.values.withIndex()) {
                    if (v.isNaN() || v.isInfinite()) { pen = false; continue }
                    val x = size.width * i / (n - 1).toFloat()
                    val y = size.height * (1f - (v - lo) / (hi - lo))
                    if (!pen) { path.moveTo(x, y); pen = true } else path.lineTo(x, y)
                }
                drawPath(path, s.color, style = Stroke(width = 3f))
            }
        }
    }
}
