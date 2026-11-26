package com.remoteview.client.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.widget.Toast
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
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
import androidx.compose.runtime.remember
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
fun LogScreen(onBack: () -> Unit) {
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
        if (lines.isEmpty()) {
            Text(stringResource(R.string.log_empty), modifier = Modifier.padding(padding).padding(16.dp))
        } else {
            SelectionContainer {
                LazyColumn(
                    state = listState,
                    modifier = Modifier.fillMaxSize().padding(padding).padding(horizontal = 8.dp),
                ) {
                    items(lines) { l ->
                        Text(l, fontFamily = FontFamily.Monospace, fontSize = 11.sp, style = MaterialTheme.typography.bodySmall)
                    }
                }
            }
        }
    }
}
