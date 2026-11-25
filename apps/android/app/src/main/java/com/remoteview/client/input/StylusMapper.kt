package com.remoteview.client.input

import android.view.MotionEvent
import android.view.View
import kotlin.math.cos
import kotlin.math.roundToInt
import kotlin.math.sin

// UNBUILT: chưa biên dịch.

/**
 * S Pen -> POINTER_PEN_INFO: áp lực 0..1024, nghiêng -90..90 độ, hover, nút bên (barrel), tẩy.
 * Gọi requestUnbufferedDispatch để không gom lô và phát lại lịch sử (historical) để nét mượt.
 */
class StylusMapper(private val sink: InputSink) {

    /** @return true nếu sự kiện là của bút và đã xử lý. */
    fun onTouch(view: View, ev: MotionEvent): Boolean {
        val tool = ev.getToolType(0)
        if (tool != MotionEvent.TOOL_TYPE_STYLUS && tool != MotionEvent.TOOL_TYPE_ERASER) return false
        if (ev.actionMasked == MotionEvent.ACTION_DOWN) view.requestUnbufferedDispatch(ev)

        when (ev.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_MOVE -> {
                for (h in 0 until ev.historySize) emit(ev, h, view, inRange = true, inContact = true)
                emit(ev, null, view, inRange = true, inContact = true)
            }
            MotionEvent.ACTION_UP -> emit(ev, null, view, inRange = true, inContact = false)
            MotionEvent.ACTION_CANCEL -> emit(ev, null, view, inRange = false, inContact = false)
        }
        return true
    }

    fun onHover(view: View, ev: MotionEvent): Boolean {
        val tool = ev.getToolType(0)
        if (tool != MotionEvent.TOOL_TYPE_STYLUS && tool != MotionEvent.TOOL_TYPE_ERASER) return false
        when (ev.actionMasked) {
            MotionEvent.ACTION_HOVER_ENTER, MotionEvent.ACTION_HOVER_MOVE ->
                emit(ev, null, view, inRange = true, inContact = false)
            MotionEvent.ACTION_HOVER_EXIT -> emit(ev, null, view, inRange = false, inContact = false)
            else -> return false
        }
        return true
    }

    private fun axis(ev: MotionEvent, axis: Int, h: Int?): Float =
        if (h == null) ev.getAxisValue(axis, 0) else ev.getHistoricalAxisValue(axis, 0, h)

    private fun emit(ev: MotionEvent, h: Int?, view: View, inRange: Boolean, inContact: Boolean) {
        val w = view.width.takeIf { it > 0 } ?: return
        val hh = view.height.takeIf { it > 0 } ?: return
        val x = axis(ev, MotionEvent.AXIS_X, h)
        val y = axis(ev, MotionEvent.AXIS_Y, h)
        val pressure = if (inContact) (axis(ev, MotionEvent.AXIS_PRESSURE, h).coerceIn(0f, 1f) * 1024f).roundToInt() else 0
        val tilt = axis(ev, MotionEvent.AXIS_TILT, h) // rad, 0 = thẳng đứng
        val orient = axis(ev, MotionEvent.AXIS_ORIENTATION, h) // rad
        // TODO: kiểm chứng dấu tiltX/tiltY trên máy thật (Windows: tiltX + = nghiêng sang phải).
        val tiltX = Math.toDegrees((tilt * sin(orient)).toDouble()).roundToInt().coerceIn(-90, 90)
        val tiltY = Math.toDegrees((-tilt * cos(orient)).toDouble()).roundToInt().coerceIn(-90, 90)
        val barrel = (ev.buttonState and MotionEvent.BUTTON_STYLUS_PRIMARY) != 0
        val eraser = ev.getToolType(0) == MotionEvent.TOOL_TYPE_ERASER

        sink.send(
            RemoteInput.Pen(
                nx = (x / w).coerceIn(0f, 1f),
                ny = (y / hh).coerceIn(0f, 1f),
                pressure = pressure,
                tiltX = tiltX,
                tiltY = tiltY,
                inRange = inRange,
                inContact = inContact,
                barrelButton = barrel,
                eraser = eraser,
            ),
        )
    }
}
