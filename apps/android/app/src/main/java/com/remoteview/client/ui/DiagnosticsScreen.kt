package com.remoteview.client.ui

import android.widget.Toast
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.remoteview.client.R
import com.remoteview.client.diag.CheckResult
import com.remoteview.client.diag.DiagReport
import com.remoteview.client.diag.DiagRunner
import com.remoteview.client.diag.UsbStateMonitor
import com.remoteview.client.diag.Verdict
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

// UNBUILT: chưa biên dịch.

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DiagnosticsScreen(onBack: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val partial = remember { mutableStateListOf<CheckResult>() }
    var report by remember { mutableStateOf<DiagReport?>(null) }
    var running by remember { mutableStateOf(false) }
    var savedPath by remember { mutableStateOf<String?>(null) }
    var usbNote by remember { mutableStateOf<String?>(null) }
    var job by remember { mutableStateOf<Job?>(null) }
    var reportText by remember { mutableStateOf("") }

    fun runDiag() {
        job?.cancel()
        job = scope.launch {
            running = true
            partial.clear()
            savedPath = null
            try {
                val r = withContext(Dispatchers.IO) {
                    DiagRunner.runAll(context) { partial.add(it) }
                }
                val text = DiagRunner.buildText(context, r)
                val path = withContext(Dispatchers.IO) { DiagRunner.save(context, r, text) }
                reportText = text
                report = r
                savedPath = path ?: context.getString(R.string.diag_save_failed)
            } finally {
                running = false
            }
        }
    }

    // Chạy lần đầu + tự chạy lại khi cắm/rút USB.
    LaunchedEffect(Unit) { runDiag() }
    val usbEvt = context.getString(R.string.diag_usb_changed)
    DisposableEffect(Unit) {
        var lastConnected: Boolean? = null
        var pending: Job? = null
        val monitor = UsbStateMonitor(context) { snap, initial ->
            val changed = snap.connected != lastConnected
            lastConnected = snap.connected
            if (!initial && changed) {
                usbNote = usbEvt
                pending?.cancel()
                pending = scope.launch {
                    delay(1200) // chờ hệ thống ổn định function USB
                    runDiag()
                }
            }
        }
        monitor.start()
        onDispose { monitor.stop(); pending?.cancel() }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.diag_title)) },
                navigationIcon = { TextButton(onClick = onBack) { Text(stringResource(R.string.diag_back)) } },
            )
        },
    ) { padding ->
        LazyColumn(
            modifier = Modifier.fillMaxSize().padding(padding).padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            item {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(stringResource(R.string.diag_instruction), style = MaterialTheme.typography.bodyMedium)
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Button(onClick = { runDiag() }, enabled = !running) {
                            Text(stringResource(if (running) R.string.diag_running else R.string.diag_rerun))
                        }
                        OutlinedButton(
                            enabled = report != null,
                            onClick = {
                                DiagRunner.copy(context, reportText)
                                Toast.makeText(context, R.string.diag_copied, Toast.LENGTH_SHORT).show()
                            },
                        ) { Text(stringResource(R.string.diag_copy)) }
                        OutlinedButton(
                            enabled = report != null,
                            onClick = { try { DiagRunner.share(context, reportText) } catch (_: Exception) {} },
                        ) { Text(stringResource(R.string.diag_share)) }
                    }
                    usbNote?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
                    savedPath?.let { Text(stringResource(R.string.diag_saved_to, it), style = MaterialTheme.typography.bodySmall) }
                }
            }
            report?.takeIf { !running }?.let { r -> item(key = "summary") { CheckCard(r.summary, big = true) } }
            val shown = if (running) partial.toList() else (report?.checks ?: partial.toList())
            items(shown, key = { it.id }) { CheckCard(it) }
        }
    }
}

@Composable
private fun CheckCard(r: CheckResult, big: Boolean = false) {
    val container = when (r.verdict) {
        Verdict.OK -> MaterialTheme.colorScheme.primaryContainer
        Verdict.WARN -> MaterialTheme.colorScheme.tertiaryContainer
        Verdict.FAIL -> MaterialTheme.colorScheme.errorContainer
        Verdict.INFO -> MaterialTheme.colorScheme.surfaceVariant
    }
    val icon = when (r.verdict) {
        Verdict.OK -> "✅"
        Verdict.WARN -> "⚠️"
        Verdict.FAIL -> "❌"
        Verdict.INFO -> "ℹ️"
    }
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = container, contentColor = MaterialTheme.colorScheme.onSurface),
    ) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(
                "$icon ${r.title}",
                style = if (big) MaterialTheme.typography.titleLarge else MaterialTheme.typography.titleMedium,
            )
            Text(r.headline, style = MaterialTheme.typography.bodyLarge)
            if (r.details.isNotEmpty()) {
                Text(
                    r.details.joinToString("\n"),
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = FontFamily.Monospace,
                    fontSize = 12.sp,
                    color = Color.Unspecified,
                )
            }
            r.explanation?.let { Text(stringResource(R.string.diag_why_label, it), style = MaterialTheme.typography.bodyMedium) }
            r.fix?.let { Text(stringResource(R.string.diag_fix_label, it), style = MaterialTheme.typography.bodyMedium) }
        }
    }
}
