@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)

package com.remoteview.client.ui

import android.widget.Toast
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.background
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.BugReport
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.Computer
import androidx.compose.material.icons.rounded.Description
import androidx.compose.material.icons.rounded.History
import androidx.compose.material.icons.rounded.QrCodeScanner
import androidx.compose.material.icons.rounded.Speed
import androidx.compose.material.icons.rounded.Usb
import androidx.compose.material.icons.rounded.Wifi
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import com.remoteview.client.R
import com.remoteview.client.update.UpdateController

data class LanHost(val name: String, val address: String)

/** Hộp thoại PIN ghép đôi USB; [error] != null: lần trước lỗi (sai PIN / host không trả lời). */
data class PinPrompt(val error: String?)

/** Máy tính (host AOA) đang cắm cáp USB. [description] = tên máy Windows do host đặt. */
data class UsbHostInfo(val description: String, val serial: String?)

/** Trạng thái thẻ chính ở đầu trang chủ. */
enum class HeroState { NO_CABLE, DETECTED, CONNECTING, DISPLAYING }

@Composable
fun HomeScreen(
    lanHosts: List<LanHost>,
    onConnect: (target: String) -> Unit,
    widthClass: WidthClass,
    update: UpdateController,
    recentHosts: List<LanHost> = emptyList(),
    usbOtp: String = "",
    pinPrompt: PinPrompt? = null,
    onUsbDisplay: () -> Unit = {},
    onPinSubmit: (String) -> Unit = {},
    onPinDismiss: () -> Unit = {},
    onDiagnostics: () -> Unit = {},
    onBench: () -> Unit = {},
    usbHost: UsbHostInfo? = null,
    usbBusy: Boolean = false,
    usbError: String? = null,
    onUsbConnect: () -> Unit = {},
    onLog: () -> Unit = {},
    contentPadding: PaddingValues = PaddingValues(),
) {
    var id by rememberSaveable { mutableStateOf("") }
    val context = LocalContext.current
    val qrTodo = stringResource(R.string.home_qr_todo)
    val idFocus = remember { FocusRequester() }

    if (pinPrompt != null) PinDialog(pinPrompt, usbOtp, onPinSubmit, onPinDismiss)

    val heroState = when {
        usbBusy -> HeroState.CONNECTING
        usbHost != null -> HeroState.DETECTED
        else -> HeroState.NO_CABLE
    }

    val hero: @Composable () -> Unit = {
        HeroCard(
            state = heroState,
            hostName = usbHost?.description?.ifBlank { null } ?: "?",
            error = usbError,
            onConnect = if (usbHost != null) onUsbConnect else onUsbDisplay,
        )
    }
    val tools: @Composable () -> Unit = {
        ToolsGrid(
            onLan = { idFocus.requestFocus() },
            onDiag = onDiagnostics,
            onBench = onBench,
            onLog = onLog,
        )
    }
    val devices: @Composable () -> Unit = {
        DevicesPane(
            id = id,
            onIdChange = { id = it },
            idFocus = idFocus,
            lanHosts = lanHosts,
            recentHosts = recentHosts,
            onConnect = onConnect,
            onQr = { Toast.makeText(context, qrTodo, Toast.LENGTH_SHORT).show() },
        )
    }

    if (widthClass == WidthClass.COMPACT) {
        Column(
            modifier = Modifier.fillMaxSize().padding(contentPadding).verticalScroll(rememberScrollState())
                .padding(horizontal = Spacing.m, vertical = Spacing.m),
            verticalArrangement = Arrangement.spacedBy(Spacing.m),
        ) {
            Header()
            UpdateBanner(update)
            hero()
            SectionTitle(R.string.home_tools)
            tools()
            devices()
        }
    } else {
        Column(
            modifier = Modifier.fillMaxSize().padding(contentPadding)
                .padding(horizontal = Spacing.l, vertical = Spacing.m),
            verticalArrangement = Arrangement.spacedBy(Spacing.m),
        ) {
            Header()
            UpdateBanner(update, Modifier.widthIn(max = 960.dp))
            Row(Modifier.weight(1f), horizontalArrangement = Arrangement.spacedBy(Spacing.l)) {
                // Trái: thẻ kết nối + công cụ. Phải: danh sách máy.
                Column(
                    Modifier.weight(1f).fillMaxSize().verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(Spacing.m),
                ) {
                    hero()
                    SectionTitle(R.string.home_tools)
                    tools()
                }
                Column(
                    Modifier.weight(1f).fillMaxSize().verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(Spacing.m),
                ) { devices() }
            }
        }
    }
}

@Composable
private fun Header() {
    Text(
        stringResource(R.string.home_title),
        style = MaterialTheme.typography.headlineMedium,
        color = MaterialTheme.colorScheme.onSurface,
        modifier = Modifier.padding(top = Spacing.s),
    )
}

@Composable
private fun SectionTitle(res: Int) {
    Text(
        stringResource(res),
        style = MaterialTheme.typography.titleMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
private fun HeroCard(state: HeroState, hostName: String, error: String?, onConnect: () -> Unit) {
    val status = LocalStatusColors.current
    val cs = MaterialTheme.colorScheme
    val container: Color
    val onContainer: Color
    val icon: ImageVector
    when (state) {
        HeroState.NO_CABLE -> { container = cs.surfaceContainerHigh; onContainer = cs.onSurface; icon = Icons.Rounded.Usb }
        HeroState.DETECTED -> { container = status.successContainer; onContainer = status.onSuccessContainer; icon = Icons.Rounded.CheckCircle }
        HeroState.CONNECTING -> { container = cs.primaryContainer; onContainer = cs.onPrimaryContainer; icon = Icons.Rounded.Usb }
        HeroState.DISPLAYING -> { container = status.successContainer; onContainer = status.onSuccessContainer; icon = Icons.Rounded.Computer }
    }
    val title = when (state) {
        HeroState.NO_CABLE -> stringResource(R.string.hero_nocable)
        HeroState.DETECTED -> stringResource(R.string.hero_detected, hostName)
        HeroState.CONNECTING -> stringResource(R.string.hero_connecting)
        HeroState.DISPLAYING -> stringResource(R.string.hero_displaying)
    }
    val sub = when (state) {
        HeroState.NO_CABLE -> stringResource(R.string.hero_nocable_sub)
        HeroState.DETECTED -> stringResource(R.string.hero_detected_sub)
        HeroState.CONNECTING -> stringResource(R.string.hero_connecting_sub)
        HeroState.DISPLAYING -> stringResource(R.string.hero_displaying_sub)
    }
    Card(
        modifier = Modifier.fillMaxWidth(),
        shape = MaterialTheme.shapes.extraLarge,
        colors = CardDefaults.cardColors(containerColor = container, contentColor = onContainer),
    ) {
        Column(
            Modifier.padding(Spacing.l),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(Spacing.m),
        ) {
            Box(
                Modifier.size(88.dp).background(onContainer.copy(alpha = 0.12f), CircleShape),
                contentAlignment = Alignment.Center,
            ) {
                if (state == HeroState.CONNECTING) {
                    CircularProgressIndicator(Modifier.size(48.dp), color = onContainer, strokeWidth = 4.dp)
                } else {
                    Icon(icon, contentDescription = null, modifier = Modifier.size(48.dp))
                }
            }
            Text(title, style = MaterialTheme.typography.headlineSmall, textAlign = TextAlign.Center)
            Text(sub, style = MaterialTheme.typography.bodyLarge, textAlign = TextAlign.Center)
            if (error != null) {
                Text(
                    stringResource(R.string.usb_card_error, error),
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodyMedium,
                    textAlign = TextAlign.Center,
                )
            }
            Button(
                onClick = onConnect,
                enabled = state != HeroState.CONNECTING && state != HeroState.DISPLAYING,
                modifier = Modifier.fillMaxWidth().heightIn(min = 64.dp),
            ) {
                Text(
                    stringResource(if (state == HeroState.CONNECTING) R.string.usb_card_connecting else R.string.hero_button),
                    style = MaterialTheme.typography.titleLarge,
                )
            }
        }
    }
}

@Composable
private fun ToolsGrid(onLan: () -> Unit, onDiag: () -> Unit, onBench: () -> Unit, onLog: () -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(Spacing.s)) {
        Row(horizontalArrangement = Arrangement.spacedBy(Spacing.s)) {
            ToolCard(Icons.Rounded.Wifi, R.string.tool_lan, R.string.tool_lan_sub, onLan, Modifier.weight(1f))
            ToolCard(Icons.Rounded.BugReport, R.string.tool_diag, R.string.tool_diag_sub, onDiag, Modifier.weight(1f))
        }
        Row(horizontalArrangement = Arrangement.spacedBy(Spacing.s)) {
            ToolCard(Icons.Rounded.Speed, R.string.tool_bench, R.string.tool_bench_sub, onBench, Modifier.weight(1f))
            ToolCard(Icons.Rounded.Description, R.string.tool_log, R.string.tool_log_sub, onLog, Modifier.weight(1f))
        }
    }
}

@Composable
private fun ToolCard(icon: ImageVector, title: Int, sub: Int, onClick: () -> Unit, modifier: Modifier) {
    Card(
        onClick = onClick,
        modifier = modifier.heightIn(min = 128.dp),
        colors = CardDefaults.cardColors(
            containerColor = MaterialTheme.colorScheme.secondaryContainer,
            contentColor = MaterialTheme.colorScheme.onSecondaryContainer,
        ),
    ) {
        Column(Modifier.padding(Spacing.m), verticalArrangement = Arrangement.spacedBy(Spacing.xs)) {
            Icon(icon, contentDescription = null, modifier = Modifier.size(28.dp))
            Text(stringResource(title), style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = Spacing.s))
            Text(stringResource(sub), style = MaterialTheme.typography.bodySmall)
        }
    }
}

@Composable
private fun DevicesPane(
    id: String,
    onIdChange: (String) -> Unit,
    idFocus: FocusRequester,
    lanHosts: List<LanHost>,
    recentHosts: List<LanHost>,
    onConnect: (String) -> Unit,
    onQr: () -> Unit,
) {
    // Kết nối thủ công (ID máy tính)
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow),
    ) {
        Column(Modifier.padding(Spacing.m), verticalArrangement = Arrangement.spacedBy(Spacing.s)) {
            Text(stringResource(R.string.home_manual_title), style = MaterialTheme.typography.titleMedium)
            OutlinedTextField(
                value = id,
                onValueChange = onIdChange,
                label = { Text(stringResource(R.string.home_id_hint)) },
                singleLine = true,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Go),
                keyboardActions = KeyboardActions(onGo = { if (id.isNotBlank()) onConnect(id.trim()) }),
                modifier = Modifier.fillMaxWidth().focusRequester(idFocus),
                trailingIcon = {
                    IconButton(onClick = onQr) {
                        Icon(Icons.Rounded.QrCodeScanner, contentDescription = stringResource(R.string.home_scan_qr))
                    }
                },
            )
            Button(
                onClick = { if (id.isNotBlank()) onConnect(id.trim()) },
                enabled = id.isNotBlank(),
                modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp),
            ) { Text(stringResource(R.string.home_connect_button)) }
        }
    }

    HostListCard(R.string.home_lan_hosts, Icons.Rounded.Wifi, lanHosts, R.string.home_lan_empty, onConnect)
    HostListCard(R.string.home_recent, Icons.Rounded.History, recentHosts, R.string.home_recent_empty, onConnect)
}

@Composable
private fun HostListCard(title: Int, icon: ImageVector, hosts: List<LanHost>, emptyRes: Int, onConnect: (String) -> Unit) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow),
    ) {
        Column(Modifier.padding(vertical = Spacing.s)) {
            Text(
                stringResource(title),
                style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.padding(horizontal = Spacing.m, vertical = Spacing.s),
            )
            if (hosts.isEmpty()) {
                Text(
                    stringResource(emptyRes),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = Spacing.m, vertical = Spacing.s),
                )
            } else {
                hosts.forEach { host ->
                    Card(
                        onClick = { onConnect(host.address) },
                        shape = MaterialTheme.shapes.medium,
                        colors = CardDefaults.cardColors(containerColor = Color.Transparent),
                        modifier = Modifier.fillMaxWidth().heightIn(min = 56.dp),
                    ) {
                        ListItem(
                            colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                            leadingContent = { Icon(icon, contentDescription = null) },
                            headlineContent = { Text(host.name) },
                            supportingContent = { Text(host.address) },
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun PinDialog(
    pinPrompt: PinPrompt,
    usbOtp: String,
    onPinSubmit: (String) -> Unit,
    onPinDismiss: () -> Unit,
) {
    var pin by rememberSaveable { mutableStateOf(usbOtp) }
    val valid = pin.length in 6..9 && pin.all { it in '0'..'9' }
    AlertDialog(
        onDismissRequest = onPinDismiss,
        title = { Text(stringResource(R.string.usb_pin_title)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(Spacing.s)) {
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
