package com.remoteview.client.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.widget.Toast
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.material3.OutlinedButton
import com.remoteview.client.LogShare
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.remoteview.client.R
import com.remoteview.client.RvLog

/** "Nhật ký kết nối": các dòng log gần nhất của app, chọn/sao chép được. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LogScreen(onBack: () -> Unit, sessionHandle: () -> Long = { 0L }) {
    val ctx = LocalContext.current
    val ver by RvLog.version.collectAsState()
    val lines = remember(ver) { RvLog.lines() }
    val listState = rememberLazyListState()
    val copiedMsg = stringResource(R.string.log_copied)
    LaunchedEffect(lines.size) { if (lines.isNotEmpty()) listState.scrollToItem(lines.size - 1) }
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.log_title)) },
                navigationIcon = { TextButton(onClick = onBack) { Text(stringResource(R.string.log_back)) } },
                actions = {
                    Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                        TextButton(onClick = {
                            val cm = ctx.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                            cm.setPrimaryClip(ClipData.newPlainText("RemoteView log", RvLog.text()))
                            Toast.makeText(ctx, copiedMsg, Toast.LENGTH_SHORT).show()
                        }) { Text(stringResource(R.string.log_copy)) }
                        TextButton(onClick = { RvLog.clear() }) { Text(stringResource(R.string.log_clear)) }
                    }
                },
            )
        },
    ) { padding ->
        val shareFail = stringResource(R.string.log_share_fail)
        val upOk = stringResource(R.string.log_upload_ok)
        val upFail = stringResource(R.string.log_upload_fail)
        val needSession = stringResource(R.string.log_upload_need_session)
        var uploading by remember { mutableStateOf(false) }
        val connected = sessionHandle() != 0L
        Column(Modifier.fillMaxSize().padding(padding)) {
            Row(
                Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                OutlinedButton(onClick = {
                    try { LogShare.share(ctx) } catch (t: Throwable) {
                        RvLog.w("chia sẻ log lỗi", t)
                        Toast.makeText(ctx, shareFail, Toast.LENGTH_SHORT).show()
                    }
                }) { Text(stringResource(R.string.log_share)) }
                OutlinedButton(
                    enabled = connected && !uploading,
                    onClick = {
                        val h = sessionHandle()
                        if (h == 0L) {
                            Toast.makeText(ctx, needSession, Toast.LENGTH_SHORT).show()
                        } else {
                            uploading = true
                            Thread({
                                val ok = LogShare.upload(h)
                                android.os.Handler(android.os.Looper.getMainLooper()).post {
                                    uploading = false
                                    Toast.makeText(ctx, if (ok) upOk else upFail, Toast.LENGTH_SHORT).show()
                                }
                            }, "rv-log-upload").also { it.isDaemon = true; it.start() }
                        }
                    },
                ) { Text(stringResource(R.string.log_upload)) }
            }
            if (lines.isEmpty()) {
                Text(stringResource(R.string.log_empty), modifier = Modifier.padding(16.dp))
            } else {
                SelectionContainer {
                    LazyColumn(
                        state = listState,
                        modifier = Modifier.fillMaxSize().padding(horizontal = 8.dp),
                    ) {
                        items(lines) { l ->
                            Text(l, fontFamily = FontFamily.Monospace, fontSize = 11.sp, style = MaterialTheme.typography.bodySmall)
                        }
                    }
                }
            }
        }
    }
}
