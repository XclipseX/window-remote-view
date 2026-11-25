package com.remoteview.client.input

import android.view.MotionEvent
import android.view.View
import kotlin.math.roundToInt

// UNBUILT: chưa biên dịch.

/**
 * Chế độ "Màn hình phụ (USB)": cảm ứng trực tiếp 1:1, tuyệt đối chuẩn hóa 0..65535,
 * tối đa 10 điểm chạm gửi dưới dạng sự kiện touch (không phải chuột). Không có cử chỉ chuột/zoom cục bộ.
 * Bút S Pen và chuột vẫn do StylusMapper / PointerCaptureController xử lý (trả false ở đây).
 */
class DirectTouchMapper(private val sink: InputSink) {

    /** @return true nếu sự kiện là cảm ứng ngón tay và đã xử lý. */
    fun onTouchEvent(view: View, ev: MotionEvent): Boolean {
        val tool = ev.getToolType(0)
        if (tool == MotionEvent.TOOL_TYPE_STYLUS || tool == MotionEvent.TOOL_TYPE_ERASER ||
            tool == MotionEvent.TOOL_TYPE_MOUSE
        ) return false
        val w = view.width
        val h = view.height
        if (w <= 0 || h <= 0) return true

        when (ev.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                view.requestUnbufferedDispatch(ev)
                emit(ev, ev.actionIndex, TouchPhase.DOWN, w, h)
            }
            MotionEvent.ACTION_POINTER_DOWN -> emit(ev, ev.actionIndex, TouchPhase.DOWN, w, h)
            MotionEvent.ACTION_MOVE -> for (i in 0 until ev.pointerCount) emit(ev, i, TouchPhase.MOVE, w, h)
            MotionEvent.ACTION_POINTER_UP, MotionEvent.ACTION_UP -> emit(ev, ev.actionIndex, TouchPhase.UP, w, h)
            MotionEvent.ACTION_CANCEL -> for (i in 0 until ev.pointerCount) emit(ev, i, TouchPhase.CANCEL, w, h)
        }
        return true
    }

    private fun emit(ev: MotionEvent, index: Int, phase: TouchPhase, w: Int, h: Int) {
        if (index < 0 || index >= ev.pointerCount) return
        val id = ev.getPointerId(index)
        if (id >= MAX_POINTS) return
        val t = ev.getToolType(index)
        if (t == MotionEvent.TOOL_TYPE_STYLUS || t == MotionEvent.TOOL_TYPE_ERASER) return
        val nx = (ev.getX(index) / w).coerceIn(0f, 1f)
        val ny = (ev.getY(index) / h).coerceIn(0f, 1f)
        val pressure = if (phase == TouchPhase.UP || phase == TouchPhase.CANCEL) 0
        else (ev.getPressure(index).coerceIn(0f, 1f) * 1024f).roundToInt()
        sink.send(
            RemoteInput.Touch(
                id = id,
                x = (nx * NORM_MAX).roundToInt(),
                y = (ny * NORM_MAX).roundToInt(),
                phase = phase,
                pressure = pressure,
            ),
        )
    }

    companion object {
        const val MAX_POINTS = 10
        const val NORM_MAX = 65535
    }
}
