@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)

package com.remoteview.client.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.OpenInBrowser
import androidx.compose.material.icons.rounded.SystemUpdate
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.remoteview.client.R
import com.remoteview.client.update.UpdateController
import com.remoteview.client.update.UpdateState

private fun shortSha(s: String) = if (s.length > 7) s.take(7) else s

/** Hộp thoại "Có bản cập nhật" (hiện sau khi kiểm tra thấy bản mới). */
@Composable
fun UpdateDialog(update: UpdateController) {
    val s = update.state
    if (!update.dialogVisible || s !is UpdateState.Available) return
    AlertDialog(
        onDismissRequest = { update.dismissDialog() },
        icon = { Icon(Icons.Rounded.SystemUpdate, contentDescription = null) },
        title = { Text(stringResource(R.string.update_available_title)) },
        text = {
            Text(stringResource(R.string.update_available_body, shortSha(s.info.remoteSha), shortSha(update.currentSha)))
        },
        confirmButton = {
            TextButton(onClick = { update.startUpdate(s.info) }) { Text(stringResource(R.string.update_now)) }
        },
        dismissButton = {
            TextButton(onClick = { update.dismissDialog() }) { Text(stringResource(R.string.update_later)) }
        },
    )
}

/** Banner đầu trang chủ khi có bản mới hoặc đang tải/cài. */
@Composable
fun UpdateBanner(update: UpdateController, modifier: Modifier = Modifier) {
    val s = update.state
    when (s) {
        is UpdateState.Available -> Card(
            onClick = { update.startUpdate(s.info) },
            modifier = modifier.fillMaxWidth(),
            colors = CardDefaults.cardColors(
                containerColor = MaterialTheme.colorScheme.primaryContainer,
                contentColor = MaterialTheme.colorScheme.onPrimaryContainer,
            ),
        ) {
            Row(
                Modifier.padding(Spacing.m),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(Spacing.m),
            ) {
                Icon(Icons.Rounded.SystemUpdate, contentDescription = null, modifier = Modifier.size(32.dp))
                Column(Modifier.weight(1f)) {
                    Text(stringResource(R.string.update_banner_title), style = MaterialTheme.typography.titleMedium)
                    Text(stringResource(R.string.update_banner_sub), style = MaterialTheme.typography.bodyMedium)
                }
            }
        }
        is UpdateState.Downloading, is UpdateState.Installing, is UpdateState.NeedsPermission ->
            Card(modifier = modifier.fillMaxWidth()) {
                Column(Modifier.padding(Spacing.m)) { UpdateStatusBody(update) }
            }
        else -> {}
    }
}

/** Phần trạng thái + nút cho cập nhật, dùng trong Cài đặt > Giới thiệu và banner. */
@Composable
fun UpdateStatusBody(update: UpdateController) {
    val s = update.state
    Column(verticalArrangement = Arrangement.spacedBy(Spacing.s)) {
        when (s) {
            UpdateState.Idle -> {}
            UpdateState.Checking -> Row(verticalAlignment = Alignment.CenterVertically) {
                CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                Text(
                    stringResource(R.string.update_checking),
                    modifier = Modifier.padding(start = Spacing.m),
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
            UpdateState.UpToDate ->
                Text(stringResource(R.string.update_uptodate), style = MaterialTheme.typography.bodyMedium)
            is UpdateState.Available -> {
                Text(
                    stringResource(R.string.update_available_body, shortSha(s.info.remoteSha), shortSha(update.currentSha)),
                    style = MaterialTheme.typography.bodyMedium,
                )
                Button(onClick = { update.startUpdate(s.info) }) {
                    Icon(Icons.Rounded.SystemUpdate, contentDescription = null, modifier = Modifier.size(18.dp))
                    Text(stringResource(R.string.update_now), modifier = Modifier.padding(start = Spacing.s))
                }
            }
            is UpdateState.Downloading -> {
                Text(
                    if (s.progress >= 0f) stringResource(R.string.update_downloading, (s.progress * 100).toInt())
                    else stringResource(R.string.update_downloading_unknown),
                    style = MaterialTheme.typography.bodyMedium,
                )
                if (s.progress >= 0f) LinearProgressIndicator(progress = { s.progress }, modifier = Modifier.fillMaxWidth())
                else LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
                TextButton(onClick = { update.cancel() }) { Text(stringResource(R.string.update_cancel)) }
            }
            is UpdateState.NeedsPermission -> {
                Text(stringResource(R.string.update_need_permission), style = MaterialTheme.typography.bodyMedium)
                Button(onClick = { update.openUnknownSourcesSettings() }) {
                    Text(stringResource(R.string.update_open_settings))
                }
            }
            is UpdateState.Installing -> Row(verticalAlignment = Alignment.CenterVertically) {
                CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                Text(
                    stringResource(R.string.update_installing),
                    modifier = Modifier.padding(start = Spacing.m),
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
            UpdateState.NotFound -> {
                Text(stringResource(R.string.update_private), style = MaterialTheme.typography.bodyMedium)
                OpenPageButton(update)
            }
            is UpdateState.Failed -> {
                Text(
                    stringResource(R.string.update_failed, s.message),
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodyMedium,
                )
                OpenPageButton(update)
            }
        }
    }
}

@Composable
private fun OpenPageButton(update: UpdateController) {
    OutlinedButton(onClick = { update.openReleasePage() }) {
        Icon(Icons.Rounded.OpenInBrowser, contentDescription = null, modifier = Modifier.size(18.dp))
        Text(stringResource(R.string.update_open_page), modifier = Modifier.padding(start = Spacing.s))
    }
}
