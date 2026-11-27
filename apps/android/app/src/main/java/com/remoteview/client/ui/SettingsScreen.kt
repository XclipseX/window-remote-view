@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)

package com.remoteview.client.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.BugReport
import androidx.compose.material.icons.rounded.Info
import androidx.compose.material.icons.rounded.Keyboard
import androidx.compose.material.icons.rounded.Memory
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.Usb
import androidx.compose.material.icons.rounded.Videocam
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.remoteview.client.BuildConfig
import com.remoteview.client.R
import com.remoteview.client.StreamSettings
import com.remoteview.client.decoder.CodecPreference
import com.remoteview.client.input.TouchMode
import com.remoteview.client.update.UpdateController
import com.remoteview.client.update.UpdateState

private enum class Category(val titleRes: Int, val subRes: Int, val icon: ImageVector) {
    VIDEO(R.string.settings_cat_video, R.string.settings_cat_video_sub, Icons.Rounded.Videocam),
    DECODE(R.string.settings_cat_decode, R.string.settings_cat_decode_sub, Icons.Rounded.Memory),
    INPUT(R.string.settings_cat_input, R.string.settings_cat_input_sub, Icons.Rounded.Keyboard),
    USB(R.string.settings_cat_usb, R.string.settings_cat_usb_sub, Icons.Rounded.Usb),
    DEBUG(R.string.settings_cat_debug, R.string.settings_cat_debug_sub, Icons.Rounded.BugReport),
    ABOUT(R.string.settings_cat_about, R.string.settings_cat_about_sub, Icons.Rounded.Info),
}

@Composable
fun SettingsScreen(
    settings: StreamSettings,
    onChange: (StreamSettings) -> Unit,
    update: UpdateController,
    widthClass: WidthClass,
    contentPadding: PaddingValues = PaddingValues(),
) {
    if (widthClass == WidthClass.COMPACT) {
        Column(
            modifier = Modifier.fillMaxSize().padding(contentPadding).verticalScroll(rememberScrollState())
                .padding(horizontal = Spacing.m, vertical = Spacing.m),
            verticalArrangement = Arrangement.spacedBy(Spacing.s),
        ) {
            Text(
                stringResource(R.string.settings_header),
                style = MaterialTheme.typography.headlineMedium,
                modifier = Modifier.padding(top = Spacing.s),
            )
            Category.entries.forEach { cat ->
                SectionHeader(cat.titleRes)
                SettingsCard { CategoryContent(cat, settings, onChange, update) }
            }
        }
    } else {
        var selected by rememberSaveable { mutableIntStateOf(0) }
        Row(
            modifier = Modifier.fillMaxSize().padding(contentPadding).padding(Spacing.l),
            horizontalArrangement = Arrangement.spacedBy(Spacing.l),
        ) {
            Column(Modifier.width(320.dp).fillMaxSize().verticalScroll(rememberScrollState())) {
                Text(
                    stringResource(R.string.settings_header),
                    style = MaterialTheme.typography.headlineMedium,
                    modifier = Modifier.padding(bottom = Spacing.m),
                )
                Category.entries.forEachIndexed { i, cat ->
                    val isSel = i == selected
                    Surface(
                        onClick = { selected = i },
                        shape = MaterialTheme.shapes.large,
                        color = if (isSel) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent,
                        modifier = Modifier.fillMaxWidth().padding(vertical = 2.dp),
                    ) {
                        ListItem(
                            colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                            leadingContent = { Icon(cat.icon, contentDescription = null) },
                            headlineContent = { Text(stringResource(cat.titleRes)) },
                            supportingContent = { Text(stringResource(cat.subRes)) },
                        )
                    }
                }
            }
            val cat = Category.entries[selected.coerceIn(0, Category.entries.size - 1)]
            Column(
                Modifier.weight(1f).fillMaxSize().verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(Spacing.s),
            ) {
                Text(
                    stringResource(cat.titleRes),
                    style = MaterialTheme.typography.headlineSmall,
                    modifier = Modifier.padding(bottom = Spacing.s),
                )
                SettingsCard { CategoryContent(cat, settings, onChange, update) }
            }
        }
    }
}

@Composable
private fun SectionHeader(res: Int) {
    Text(
        stringResource(res),
        style = MaterialTheme.typography.titleSmall,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(start = Spacing.s, top = Spacing.m),
    )
}

@Composable
private fun SettingsCard(content: @Composable () -> Unit) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        shape = MaterialTheme.shapes.large,
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow),
    ) {
        Column(Modifier.padding(vertical = Spacing.s)) { content() }
    }
}

@Composable
private fun CategoryContent(
    cat: Category,
    settings: StreamSettings,
    onChange: (StreamSettings) -> Unit,
    update: UpdateController,
) {
    when (cat) {
        Category.VIDEO -> {
            // Độ phân giải 25..100% bước 5 (15 khoảng -> steps = 14)
            SliderRow(
                R.string.settings_res_title, R.string.settings_res_hint,
                stringResource(R.string.chip_percent, settings.resolutionPercent),
                settings.resolutionPercent.toFloat(), 25f..100f, 14,
            ) { onChange(settings.copy(resolutionPercent = (it / 5f).toInt() * 5)) }
            RowDivider()
            // FPS: 30/60/90/120 (chỉ số 0..3)
            val fpsIndex = StreamSettings.FPS_OPTIONS.indexOf(settings.fps).coerceAtLeast(0)
            SliderRow(
                R.string.settings_fps_title, R.string.settings_fps_hint,
                stringResource(R.string.chip_fps, settings.fps),
                fpsIndex.toFloat(), 0f..3f, 2,
            ) { onChange(settings.copy(fps = StreamSettings.FPS_OPTIONS[it.toInt().coerceIn(0, 3)])) }
            RowDivider()
            SliderRow(
                R.string.settings_quality_title, R.string.settings_quality_hint,
                stringResource(R.string.chip_percent, settings.qualityPercent),
                settings.qualityPercent.toFloat(), 1f..100f, 0,
            ) { onChange(settings.copy(qualityPercent = it.toInt().coerceIn(1, 100))) }
            RowDivider()
            Column(Modifier.padding(horizontal = Spacing.m, vertical = Spacing.s)) {
                Text(stringResource(R.string.settings_codec), style = MaterialTheme.typography.titleMedium)
                Text(
                    stringResource(R.string.settings_codec_hint),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Row(
                    modifier = Modifier.horizontalScroll(rememberScrollState()).padding(top = Spacing.s),
                    horizontalArrangement = Arrangement.spacedBy(Spacing.s),
                ) {
                    CodecChip(R.string.codec_auto, CodecPreference.AUTO, settings, onChange)
                    CodecChip(R.string.codec_h264, CodecPreference.H264, settings, onChange)
                    CodecChip(R.string.codec_hevc, CodecPreference.HEVC, settings, onChange)
                    CodecChip(R.string.codec_av1, CodecPreference.AV1, settings, onChange)
                }
            }
        }
        Category.DECODE -> {
            SwitchRow(R.string.settings_hw_decode, R.string.settings_hw_decode_hint, settings.hardwareDecode) {
                onChange(settings.copy(hardwareDecode = it))
            }
            if (!settings.hardwareDecode) {
                Text(
                    stringResource(R.string.settings_hw_decode_off_warning),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.error,
                    modifier = Modifier.padding(horizontal = Spacing.m, vertical = Spacing.xs),
                )
            }
            SwitchRow(R.string.settings_allow_av1, null, settings.allowAv1) { onChange(settings.copy(allowAv1 = it)) }
            SwitchRow(R.string.settings_direct_present, R.string.settings_direct_present_hint, settings.directPresent) {
                onChange(settings.copy(directPresent = it))
            }
        }
        Category.INPUT -> {
            SwitchRow(R.string.settings_raw_typing, R.string.settings_raw_typing_hint, settings.rawTyping) {
                onChange(settings.copy(rawTyping = it))
            }
            SwitchRow(R.string.settings_trackpad, null, settings.touchMode == TouchMode.TRACKPAD) {
                onChange(settings.copy(touchMode = if (it) TouchMode.TRACKPAD else TouchMode.DIRECT))
            }
            SwitchRow(R.string.settings_audio, R.string.settings_audio_hint, settings.audioEnabled) {
                onChange(settings.copy(audioEnabled = it))
            }
        }
        Category.USB -> {
            SwitchRow(R.string.settings_usb_auto, R.string.settings_usb_auto_hint, settings.autoConnectUsb) {
                onChange(settings.copy(autoConnectUsb = it))
            }
            SwitchRow(R.string.settings_usb_mod_bar, R.string.settings_usb_mod_bar_hint, settings.usbShowModifierBar) {
                onChange(settings.copy(usbShowModifierBar = it))
            }
        }
        Category.DEBUG -> {
            SwitchRow(R.string.settings_fps_hud, R.string.settings_fps_hud_hint, settings.showFpsHud) {
                onChange(settings.copy(showFpsHud = it))
            }
            RowDivider()
            SwitchRow(R.string.settings_debug_overlay, R.string.settings_debug_overlay_hint, settings.debugOverlay) {
                onChange(settings.copy(debugOverlay = it))
            }
            RowDivider()
            SwitchRow(R.string.settings_log_autosend, R.string.settings_log_autosend_hint, settings.autoSendLogOnError) {
                onChange(settings.copy(autoSendLogOnError = it))
            }
        }
        Category.ABOUT -> AboutContent(settings, onChange, update)
    }
}

@Composable
private fun AboutContent(settings: StreamSettings, onChange: (StreamSettings) -> Unit, update: UpdateController) {
    InfoRow(R.string.about_version, BuildConfig.VERSION_NAME)
    RowDivider()
    InfoRow(R.string.about_build, BuildConfig.GIT_SHA.take(12))
    RowDivider()
    SwitchRow(R.string.update_auto, R.string.update_auto_hint, settings.autoCheckUpdate) {
        onChange(settings.copy(autoCheckUpdate = it))
    }
    RowDivider()
    Column(
        Modifier.padding(horizontal = Spacing.m, vertical = Spacing.s),
        verticalArrangement = Arrangement.spacedBy(Spacing.s),
    ) {
        val busy = update.state is UpdateState.Checking || update.state is UpdateState.Downloading ||
            update.state is UpdateState.Installing
        OutlinedButton(onClick = { update.check(manual = true) }, enabled = !busy, modifier = Modifier.heightIn(min = 48.dp)) {
            Icon(Icons.Rounded.Refresh, contentDescription = null, modifier = Modifier.padding(end = Spacing.s))
            Text(stringResource(R.string.update_check))
        }
        UpdateStatusBody(update)
    }
}

@Composable
private fun InfoRow(titleRes: Int, value: String) {
    ListItem(
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        headlineContent = { Text(stringResource(titleRes)) },
        trailingContent = { ValueChip(value) },
    )
}

@Composable
private fun RowDivider() {
    HorizontalDivider(
        modifier = Modifier.padding(horizontal = Spacing.m),
        color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.5f),
    )
}

@Composable
private fun ValueChip(text: String) {
    Surface(shape = MaterialTheme.shapes.small, color = MaterialTheme.colorScheme.primaryContainer) {
        Text(
            text,
            style = MaterialTheme.typography.labelLarge,
            color = MaterialTheme.colorScheme.onPrimaryContainer,
            modifier = Modifier.padding(horizontal = Spacing.s + Spacing.xs, vertical = Spacing.xs + 2.dp),
        )
    }
}

@Composable
private fun SliderRow(
    titleRes: Int,
    hintRes: Int,
    valueText: String,
    value: Float,
    range: ClosedFloatingPointRange<Float>,
    steps: Int,
    onValueChange: (Float) -> Unit,
) {
    Column(Modifier.fillMaxWidth().padding(horizontal = Spacing.m, vertical = Spacing.s)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(stringResource(titleRes), style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
            ValueChip(valueText)
        }
        Text(
            stringResource(hintRes),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Slider(
            value = value,
            onValueChange = onValueChange,
            valueRange = range,
            steps = steps,
            modifier = Modifier.heightIn(min = 48.dp),
        )
    }
}

@Composable
private fun CodecChip(
    labelRes: Int,
    value: CodecPreference,
    settings: StreamSettings,
    onChange: (StreamSettings) -> Unit,
) {
    FilterChip(
        selected = settings.codec == value,
        onClick = { onChange(settings.copy(codec = value)) },
        label = { Text(stringResource(labelRes)) },
    )
}

@Composable
private fun SwitchRow(titleRes: Int, hintRes: Int?, checked: Boolean, onCheckedChange: (Boolean) -> Unit) {
    ListItem(
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        modifier = Modifier.heightIn(min = 64.dp).clickable { onCheckedChange(!checked) },
        headlineContent = { Text(stringResource(titleRes), style = MaterialTheme.typography.bodyLarge) },
        supportingContent = hintRes?.let { r -> { Text(stringResource(r)) } },
        trailingContent = { Switch(checked = checked, onCheckedChange = onCheckedChange) },
    )
}
