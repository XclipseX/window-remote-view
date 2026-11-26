package com.remoteview.client.ui

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.remoteview.client.R
import com.remoteview.client.StreamSettings
import com.remoteview.client.decoder.CodecPreference
import com.remoteview.client.input.TouchMode

// UNBUILT: chưa biên dịch.

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(
    settings: StreamSettings,
    onChange: (StreamSettings) -> Unit,
    onBack: () -> Unit,
) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.settings_title)) },
                navigationIcon = { TextButton(onClick = onBack) { Text(stringResource(R.string.settings_back)) } },
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier.fillMaxSize().padding(padding).padding(horizontal = 16.dp)
                .verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            // Độ phân giải 25..100% bước 5 (15 khoảng -> steps = 14)
            Text(stringResource(R.string.settings_resolution, settings.resolutionPercent))
            Slider(
                value = settings.resolutionPercent.toFloat(),
                onValueChange = { onChange(settings.copy(resolutionPercent = (it / 5f).toInt() * 5)) },
                valueRange = 25f..100f,
                steps = 14,
            )

            // FPS: 30/60/90/120 (chỉ số 0..3)
            val fpsIndex = StreamSettings.FPS_OPTIONS.indexOf(settings.fps).coerceAtLeast(0)
            Text(stringResource(R.string.settings_fps, settings.fps))
            Slider(
                value = fpsIndex.toFloat(),
                onValueChange = {
                    onChange(settings.copy(fps = StreamSettings.FPS_OPTIONS[it.toInt().coerceIn(0, 3)]))
                },
                valueRange = 0f..3f,
                steps = 2,
            )

            // Chất lượng 1..100%
            Text(stringResource(R.string.settings_quality, settings.qualityPercent))
            Slider(
                value = settings.qualityPercent.toFloat(),
                onValueChange = { onChange(settings.copy(qualityPercent = it.toInt().coerceIn(1, 100))) },
                valueRange = 1f..100f,
            )

            Text(stringResource(R.string.settings_codec), style = MaterialTheme.typography.titleSmall)
            Row(
                modifier = Modifier.horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                CodecChip(R.string.codec_auto, CodecPreference.AUTO, settings, onChange)
                CodecChip(R.string.codec_h264, CodecPreference.H264, settings, onChange)
                CodecChip(R.string.codec_hevc, CodecPreference.HEVC, settings, onChange)
                CodecChip(R.string.codec_av1, CodecPreference.AV1, settings, onChange)
            }

            SwitchRow(R.string.settings_hw_decode, R.string.settings_hw_decode_hint, settings.hardwareDecode) {
                onChange(settings.copy(hardwareDecode = it))
            }
            SwitchRow(R.string.settings_allow_av1, null, settings.allowAv1) {
                onChange(settings.copy(allowAv1 = it))
            }
            SwitchRow(R.string.settings_raw_typing, R.string.settings_raw_typing_hint, settings.rawTyping) {
                onChange(settings.copy(rawTyping = it))
            }
            SwitchRow(R.string.settings_direct_present, R.string.settings_direct_present_hint, settings.directPresent) {
                onChange(settings.copy(directPresent = it))
            }
            SwitchRow(R.string.settings_trackpad, null, settings.touchMode == TouchMode.TRACKPAD) {
                onChange(settings.copy(touchMode = if (it) TouchMode.TRACKPAD else TouchMode.DIRECT))
            }
            SwitchRow(R.string.settings_audio, R.string.settings_audio_hint, settings.audioEnabled) {
                onChange(settings.copy(audioEnabled = it))
            }
            SwitchRow(R.string.settings_usb_auto, R.string.settings_usb_auto_hint, settings.autoConnectUsb) {
                onChange(settings.copy(autoConnectUsb = it))
            }
            SwitchRow(R.string.settings_debug_overlay, R.string.settings_debug_overlay_hint, settings.debugOverlay) {
                onChange(settings.copy(debugOverlay = it))
            }
            SwitchRow(R.string.settings_usb_mod_bar, R.string.settings_usb_mod_bar_hint, settings.usbShowModifierBar) {
                onChange(settings.copy(usbShowModifierBar = it))
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
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
    Row(
        modifier = Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Column(modifier = Modifier.weight(1f).padding(end = 12.dp)) {
            Text(stringResource(titleRes))
            if (hintRes != null) {
                Text(stringResource(hintRes), style = MaterialTheme.typography.bodySmall)
            }
        }
        Switch(checked = checked, onCheckedChange = onCheckedChange)
    }
}
