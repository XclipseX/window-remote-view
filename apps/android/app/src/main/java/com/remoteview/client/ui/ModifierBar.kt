package com.remoteview.client.ui

import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.horizontalScroll
import com.remoteview.client.R
import com.remoteview.client.input.KeyboardBridge
import com.remoteview.client.input.ScanCodeMap
import kotlin.math.roundToInt
import androidx.compose.ui.unit.IntOffset

// UNBUILT: chưa biên dịch.

/** Thanh phím nổi, kéo được: Win, Alt+Tab, Ctrl+Alt+Del, Esc, PrtSc và nút bật bàn phím ảo. */
@Composable
fun ModifierBar(
    keyboard: KeyboardBridge,
    onToggleSoftKeyboard: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var dx by remember { mutableFloatStateOf(0f) }
    var dy by remember { mutableFloatStateOf(0f) }

    Surface(
        modifier = modifier
            .offset { IntOffset(dx.roundToInt(), dy.roundToInt()) }
            .padding(8.dp)
            .pointerInput(Unit) {
                detectDragGestures { change, drag ->
                    change.consume()
                    dx += drag.x
                    dy += drag.y
                }
            },
        shape = MaterialTheme.shapes.large,
        tonalElevation = 6.dp,
        color = MaterialTheme.colorScheme.surface.copy(alpha = 0.92f),
    ) {
        Row(
            modifier = Modifier.padding(6.dp).horizontalScroll(rememberScrollState()),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Key(stringResource(R.string.mod_win)) { keyboard.tap(ScanCodeMap.WIN) }
            Key(stringResource(R.string.mod_alt_tab)) {
                keyboard.chord(listOf(ScanCodeMap.L_ALT), ScanCodeMap.TAB)
            }
            Key(stringResource(R.string.mod_ctrl_alt_del)) { keyboard.sendSecureAttention() }
            Key(stringResource(R.string.mod_esc)) { keyboard.tap(ScanCodeMap.ESC) }
            Key(stringResource(R.string.mod_prtsc)) { keyboard.tap(ScanCodeMap.PRTSC) }
            Key(stringResource(R.string.mod_keyboard), onToggleSoftKeyboard)
        }
    }
}

@Composable
private fun Key(label: String, onClick: () -> Unit) {
    FilledTonalButton(onClick = onClick) { Text(label) }
}
