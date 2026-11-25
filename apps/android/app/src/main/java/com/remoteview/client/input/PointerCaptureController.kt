package com.remoteview.client.input

import android.view.MotionEvent
import android.view.View
import kotlin.math.roundToInt

// UNBUILT: chưa biên dịch.

/**
 * Chuột Bluetooth/OTG: requestPointerCapture() (API 26+) -> nhận delta tương đối trong
 * OnCapturedPointerListener. View phải có focus và cửa sổ có focus khi gọi [enable].
 */
class PointerCaptureController(private val view: View, private val sink: InputSink) {

    var enabled = false
        private set

    private var remX = 0f
    private var remY = 0f
    private var buttonsDown = 0

    fun enable() {
        if (enabled) return
        enabled = true
        view.setOnCapturedPointerListener { _, ev -> handle(ev) }
        view.requestPointerCapture()
    }

    fun disable() {
        if (!enabled) return
        enabled = false
        view.releasePointerCapture()
        view.setOnCapturedPointerListener(null)
        // Nhả các nút đang giữ để host không bị kẹt chuột.
        for (b in MouseButtonId.values()) sink.send(RemoteInput.MouseButton(b, false))
        buttonsDown = 0
    }

    /** Gọi lại khi cửa sổ lấy lại focus (capture bị mất khi mất focus). */
    fun onWindowFocusChanged(hasFocus: Boolean) {
        if (hasFocus && enabled && !view.hasPointerCapture()) view.requestPointerCapture()
    }

    private fun handle(ev: MotionEvent): Boolean {
        when (ev.actionMasked) {
            MotionEvent.ACTION_MOVE, MotionEvent.ACTION_HOVER_MOVE -> {
                // Với SOURCE_MOUSE_RELATIVE, x/y là delta.
                if ((ev.source and android.view.InputDevice.SOURCE_MOUSE_RELATIVE) == android.view.InputDevice.SOURCE_MOUSE_RELATIVE) {
                    remX += ev.x
                    remY += ev.y
                    val ix = remX.roundToInt()
                    val iy = remY.roundToInt()
                    if (ix != 0 || iy != 0) {
                        remX -= ix; remY -= iy
                        sink.send(RemoteInput.MouseMoveRel(ix, iy))
                    }
                }
            }
            MotionEvent.ACTION_BUTTON_PRESS -> button(ev.actionButton, true)
            MotionEvent.ACTION_BUTTON_RELEASE -> button(ev.actionButton, false)
            MotionEvent.ACTION_SCROLL -> {
                val v = ev.getAxisValue(MotionEvent.AXIS_VSCROLL)
                val h = ev.getAxisValue(MotionEvent.AXIS_HSCROLL)
                sink.send(RemoteInput.MouseWheel((h * 120f).roundToInt(), (v * 120f).roundToInt()))
            }
        }
        return true
    }

    private fun button(actionButton: Int, down: Boolean) {
        val id = when (actionButton) {
            MotionEvent.BUTTON_PRIMARY -> MouseButtonId.LEFT
            MotionEvent.BUTTON_SECONDARY -> MouseButtonId.RIGHT
            MotionEvent.BUTTON_TERTIARY -> MouseButtonId.MIDDLE
            MotionEvent.BUTTON_BACK -> MouseButtonId.X1
            MotionEvent.BUTTON_FORWARD -> MouseButtonId.X2
            else -> return
        }
        sink.send(RemoteInput.MouseButton(id, down))
    }
}
