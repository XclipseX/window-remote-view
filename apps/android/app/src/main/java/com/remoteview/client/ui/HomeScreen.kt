package com.remoteview.client.ui

import android.widget.Toast
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import com.remoteview.client.R

// UNBUILT: chưa biên dịch.

data class LanHost(val name: String, val address: String)

/** Hộp thoại PIN ghép đôi USB; [error] != null: lần trước lỗi (sai PIN / host không trả lời). */
data class PinPrompt(val error: String?)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HomeScreen(
    lanHosts: List<LanHost>,
    onConnect: (target: String) -> Unit,
    onOpenSettings: () -> Unit,
    usbOtp: String = "",
    pinPrompt: PinPrompt? = null,
    onUsbDisplay: () -> Unit = {},
    onPinSubmit: (String) -> Unit = {},
    onPinDismiss: () -> Unit = {},
    onDiagnostics: () -> Unit = {},
    onBench: () -> Unit = {},
) {
    var id by rememberSaveable { mutableStateOf("") }
    val context = LocalContext.current
    val qrTodo = stringResource(R.string.home_qr_todo)

    if (pinPrompt != null) {
        var pin by rememberSaveable { mutableStateOf(usbOtp) }
        val valid = pin.length in 6..9 && pin.all { it in '0'..'9' }
        AlertDialog(
            onDismissRequest = onPinDismiss,
            title = { Text(stringResource(R.string.usb_pin_title)) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(stringResource(R.string.usb_pin_hint), style = MaterialTheme.typography.bodyMedium)
                    OutlinedTextField(
                        value = pin,
                        onValueChange = { pin = it.filter { c -> c in '0'..'9' }.take(9) },
                        label = { Text(stringResource(R.string.usb_pin_label)) },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
                        isError = pinPrompt.error != null,
                        supportingText = pinPrompt.error?.let { e -> { Text(stringResource(R.string.usb_pin_error, e)) } },
                    )
                }
            },
            confirmButton = {
                TextButton(onClick = { onPinSubmit(pin) }, enabled = valid) { Text(stringResource(R.string.usb_pin_connect)) }
            },
            dismissButton = { TextButton(onClick = onPinDismiss) { Text(stringResource(R.string.usb_pin_cancel)) } },
        )
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.home_title)) },
                actions = { TextButton(onClick = onOpenSettings) { Text(stringResource(R.string.home_settings)) } },
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier.fillMaxSize().padding(padding).padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(stringResource(R.string.home_connect_to), style = MaterialTheme.typography.titleMedium)
            OutlinedTextField(
                value = id,
                onValueChange = { id = it },
                label = { Text(stringResource(R.string.home_id_hint)) },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = { if (id.isNotBlank()) onConnect(id.trim()) }, enabled = id.isNotBlank()) {
                    Text(stringResource(R.string.home_connect_button))
                }
                OutlinedButton(onClick = { Toast.makeText(context, qrTodo, Toast.LENGTH_SHORT).show() }) {
                    Text(stringResource(R.string.home_scan_qr)) // TODO: quét QR (CameraX + ML Kit)
                }
            }

            Button(onClick = onUsbDisplay, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.home_usb_display))
            }
            Text(stringResource(R.string.home_usb_display_hint), style = MaterialTheme.typography.bodySmall)

            OutlinedButton(onClick = onDiagnostics, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.home_diag))
            }
            Text(stringResource(R.string.home_diag_hint), style = MaterialTheme.typography.bodySmall)

            OutlinedButton(onClick = onBench, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.home_bench))
            }
            Text(stringResource(R.string.home_bench_hint), style = MaterialTheme.typography.bodySmall)

            Text(stringResource(R.string.home_lan_hosts), style = MaterialTheme.typography.titleMedium)
            if (lanHosts.isEmpty()) {
                Text(stringResource(R.string.home_lan_empty), style = MaterialTheme.typography.bodyMedium)
            } else {
                LazyColumn {
                    items(lanHosts) { host ->
                        ListItem(
                            headlineContent = { Text(host.name) },
                            supportingContent = { Text(host.address) },
                            modifier = Modifier.clickable { onConnect(host.address) },
                        )
                        HorizontalDivider()
                    }
                }
            }
        }
    }
}
