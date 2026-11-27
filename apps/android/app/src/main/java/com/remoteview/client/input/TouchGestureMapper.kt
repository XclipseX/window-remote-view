package com.remoteview.client.input

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.view.MotionEvent
import android.view.ViewConfiguration
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.min

enum class TouchMode {
    /** Con trỏ tương đối như touchpad (mặc định điện thoại). */
    TRACKPAD,
    /** Chạm trực tiếp tuyệt đối (mặc định máy tính bảng). */
    DIRECT,
}

/**
 * Cử chỉ: chạm 1 ngón = nhấp trái; giữ 400 ms = nhấp phải; chạm 2 ngón = nhấp phải;
 * 2 ngón rê = cuộn; chụm/mở = zoom cục bộ; chạm-đúp-giữ rồi rê = kéo thả.
 * Bút (S Pen) và chuột do StylusMapper / PointerCaptureController xử lý, không phải lớp này.
 */
class TouchGestureMapper(
    context: Context,
    private val sink: InputSink,
    var mode: TouchMode,
    /** Chế độ trackpad: delta pixel gửi đi, để UI dịch con trỏ cục bộ ngay lập tức. */
    private val onLocalMove: (dx: Int, dy: Int) -> Unit = { _, _ -> },
    /** scaleFactor tức thời (>1 = phóng to) quanh điểm (focusX, focusY) theo tọa độ hộp chứa view. */
    private val onPinchZoom: (scaleFactor: Float, focusX: Float, focusY: Float) -> Unit = { _, _, _ -> },
) {
    /** Callback mở rộng kèm dịch chuyển (panDx, panDy) trong hệ tọa độ hộp chứa, do RemoteSurfaceView gắn khi bind. */
    internal var onPinchZoomPan: ((scaleFactor: Float, focusX: Float, focusY: Float, panDx: Float, panDy: Float) -> Unit)? = null

    private val slop = ViewConfiguration.get(context).scaledTouchSlop.toFloat()
    private val density = context.resources.displayMetrics.density
    private val handler = Handler(Looper.getMainLooper())

    // 1 ngón (lưu theo tọa độ hộp chứa không bị scale/translation làm méo)
    private var downTime = 0L
    private var startX = 0f
    private var startY = 0f
    private var lastX = 0f
    private var lastY = 0f
    private var moved = false
    private var longPressFired = false
    private var dragging = false
    private var maxPointers = 0
    private var suppressSingle = false
    private var lastTapUpTime = 0L
    private var lastTapX = 0f
    private var lastTapY = 0f
    private var remX = 0f
    private var remY = 0f

    // 2 ngón (tọa độ hộp chứa bất biến với scale/translation của SurfaceView để không dao động khi zoom)
    private enum class Two { NONE, SCROLL, PINCH }
    private var two = Two.NONE
    private var twoStartCx = 0f
    private var twoStartCy = 0f
    private var twoStartDist = 0f
    private var twoLastCx = 0f
    private var twoLastCy = 0f
    private var twoLastDist = 0f
    private var twoMoved = false
    private var scrollRemX = 0f
    private var scrollRemY = 0f

    private val longPress = Runnable {
        if (!moved && maxPointers == 1 && !dragging) {
            longPressFired = true
            click(MouseButtonId.RIGHT)
        }
    }

    fun onTouchEvent(
        ev: MotionEvent,
        viewW: Int,
        viewH: Int,
        viewScale: Float = 1f,
        viewTransX: Float = 0f,
        viewTransY: Float = 0f,
    ): Boolean {
        val tool = ev.getToolType(0)
        if (tool == MotionEvent.TOOL_TYPE_STYLUS || tool == MotionEvent.TOOL_TYPE_ERASER ||
            tool == MotionEvent.TOOL_TYPE_MOUSE
        ) return false

        val scale = if (viewScale.isFinite() && viewScale > 0f) viewScale else 1f
        when (ev.actionMasked) {
            MotionEvent.ACTION_DOWN -> onDown(ev, viewW, viewH, scale, viewTransX, viewTransY)
            MotionEvent.ACTION_POINTER_DOWN -> {
                maxPointers = maxOf(maxPointers, ev.pointerCount)
                if (ev.pointerCount == 2) beginTwo(ev, scale, viewTransX, viewTransY)
            }
            MotionEvent.ACTION_MOVE ->
                if (ev.pointerCount >= 2) moveTwo(ev, scale, viewTransX, viewTransY)
                else moveSingle(ev, viewW, viewH, scale, viewTransX, viewTransY)
            MotionEvent.ACTION_POINTER_UP -> if (ev.pointerCount == 2) endTwo()
            MotionEvent.ACTION_UP -> onUp(ev, scale, viewTransX, viewTransY)
            MotionEvent.ACTION_CANCEL -> cancel()
        }
        return true
    }

    // ---------------------------------------------------------------- 1 ngón

    private fun onDown(ev: MotionEvent, w: Int, h: Int, scale: Float, transX: Float, transY: Float) {
        val now = SystemClock.uptimeMillis()
        val px = parentX(ev, 0, scale, transX)
        val py = parentY(ev, 0, scale, transY)
        downTime = now
        startX = px; startY = py; lastX = px; lastY = py
        moved = false; longPressFired = false; maxPointers = 1; suppressSingle = false
        dragging = false; two = Two.NONE; twoMoved = false; remX = 0f; remY = 0f
        handler.removeCallbacks(longPress)

        if (mode == TouchMode.DIRECT) moveAbs(ev.x, ev.y, w, h)

        val nearLastTap = hypot(px - lastTapX, py - lastTapY) < slop * 4
        if (now - lastTapUpTime < DOUBLE_TAP_MS && nearLastTap) {
            // Chạm-đúp-giữ = kéo thả: giữ chuột trái.
            dragging = true
            sink.send(RemoteInput.MouseButton(MouseButtonId.LEFT, true))
        } else {
            handler.postDelayed(longPress, LONG_PRESS_MS)
        }
    }

    private fun moveSingle(ev: MotionEvent, w: Int, h: Int, scale: Float, transX: Float, transY: Float) {
        if (suppressSingle) return
        val px = parentX(ev, 0, scale, transX)
        val py = parentY(ev, 0, scale, transY)
        if (!moved && hypot(px - startX, py - startY) > slop) {
            moved = true
            handler.removeCallbacks(longPress)
        }
        if (!moved && !dragging) return
        when (mode) {
            TouchMode.DIRECT -> moveAbs(ev.x, ev.y, w, h)
            TouchMode.TRACKPAD -> {
                val dx = px - lastX
                val dy = py - lastY
                // Gia tốc đơn giản: chuyển động nhanh thì khuếch đại tới 3x.
                val gain = 1f + min(2f, hypot(dx, dy) / (12f * density))
                remX += dx * gain
                remY += dy * gain
                val ix = remX.toInt()
                val iy = remY.toInt()
                if (ix != 0 || iy != 0) {
                    remX -= ix; remY -= iy
                    sink.send(RemoteInput.MouseMoveRel(ix, iy))
                    onLocalMove(ix, iy)
                }
            }
        }
        lastX = px; lastY = py
    }

    private fun onUp(ev: MotionEvent, scale: Float, transX: Float, transY: Float) {
        handler.removeCallbacks(longPress)
        val now = SystemClock.uptimeMillis()
        when {
            suppressSingle -> {}
            dragging -> {
                sink.send(RemoteInput.MouseButton(MouseButtonId.LEFT, false))
                lastTapUpTime = 0
            }
            !moved && !longPressFired && maxPointers == 1 && now - downTime < TAP_MS -> {
                click(MouseButtonId.LEFT)
                lastTapUpTime = now
                lastTapX = parentX(ev, 0, scale, transX)
                lastTapY = parentY(ev, 0, scale, transY)
            }
            else -> lastTapUpTime = 0
        }
        reset()
    }

    private fun cancel() {
        handler.removeCallbacks(longPress)
        if (dragging) sink.send(RemoteInput.MouseButton(MouseButtonId.LEFT, false))
        reset()
    }

    private fun reset() {
        dragging = false; moved = false; longPressFired = false; maxPointers = 0
        suppressSingle = false; two = Two.NONE
    }

    // ---------------------------------------------------------------- 2 ngón

    private fun beginTwo(ev: MotionEvent, scale: Float, transX: Float, transY: Float) {
        if (ev.pointerCount < 2) return
        handler.removeCallbacks(longPress)
        if (dragging) {
            sink.send(RemoteInput.MouseButton(MouseButtonId.LEFT, false))
            dragging = false
        }
        suppressSingle = true
        two = Two.NONE
        twoMoved = false
        scrollRemX = 0f; scrollRemY = 0f
        val x0 = parentX(ev, 0, scale, transX)
        val y0 = parentY(ev, 0, scale, transY)
        val x1 = parentX(ev, 1, scale, transX)
        val y1 = parentY(ev, 1, scale, transY)
        twoStartCx = (x0 + x1) / 2f
        twoStartCy = (y0 + y1) / 2f
        twoStartDist = hypot(x0 - x1, y0 - y1)
        twoLastCx = twoStartCx; twoLastCy = twoStartCy; twoLastDist = twoStartDist
    }

    private fun moveTwo(ev: MotionEvent, scale: Float, transX: Float, transY: Float) {
        if (ev.pointerCount < 2) return
        val x0 = parentX(ev, 0, scale, transX)
        val y0 = parentY(ev, 0, scale, transY)
        val x1 = parentX(ev, 1, scale, transX)
        val y1 = parentY(ev, 1, scale, transY)
        val cx = (x0 + x1) / 2f
        val cy = (y0 + y1) / 2f
        val dist = hypot(x0 - x1, y0 - y1)

        if (two == Two.NONE) {
            val centroidMove = hypot(cx - twoStartCx, cy - twoStartCy)
            val distChange = abs(dist - twoStartDist)
            if (centroidMove > slop || distChange > slop) {
                twoMoved = true
                two = if (distChange > centroidMove) Two.PINCH else Two.SCROLL
            }
        }
        when (two) {
            Two.SCROLL -> {
                // Ngón rê lên (dy<0) -> cuộn xuống -> wheel âm. Rê trái -> cuộn phải (+).
                val pxPerNotch = 40f * density
                scrollRemY += (cy - twoLastCy) / pxPerNotch * WHEEL_DELTA
                scrollRemX += -(cx - twoLastCx) / pxPerNotch * WHEEL_DELTA
                val iy = scrollRemY.toInt()
                val ix = scrollRemX.toInt()
                if (ix != 0 || iy != 0) {
                    scrollRemX -= ix; scrollRemY -= iy
                    sink.send(RemoteInput.MouseWheel(horizontal = ix, vertical = iy))
                }
            }
            Two.PINCH -> if (twoLastDist > 1f && dist > 1f) {
                val factor = dist / twoLastDist
                val panDx = cx - twoLastCx
                val panDy = cy - twoLastCy
                val cb = onPinchZoomPan
                if (cb != null) {
                    cb(factor, cx, cy, panDx, panDy)
                } else {
                    onPinchZoom(factor, cx, cy)
                }
            }
            Two.NONE -> {}
        }
        twoLastCx = cx; twoLastCy = cy; twoLastDist = dist
    }

    private fun endTwo() {
        // Chạm nhanh 2 ngón không rê = nhấp phải.
        if (!twoMoved && maxPointers == 2 && SystemClock.uptimeMillis() - downTime < TAP_MS) {
            click(MouseButtonId.RIGHT)
        }
        two = Two.NONE
        // suppressSingle vẫn true tới ACTION_UP để ngón còn lại không kéo con trỏ.
    }

    // ---------------------------------------------------------------- helpers

    private fun parentX(ev: MotionEvent, index: Int, scale: Float, transX: Float): Float =
        ev.getX(index) * scale + transX

    private fun parentY(ev: MotionEvent, index: Int, scale: Float, transY: Float): Float =
        ev.getY(index) * scale + transY

    private fun moveAbs(x: Float, y: Float, w: Int, h: Int) {
        if (w <= 0 || h <= 0) return
        sink.send(RemoteInput.MouseMoveAbs((x / w).coerceIn(0f, 1f), (y / h).coerceIn(0f, 1f)))
    }

    private fun click(b: MouseButtonId) {
        sink.send(RemoteInput.MouseButton(b, true))
        sink.send(RemoteInput.MouseButton(b, false))
    }

    companion object {
        private const val LONG_PRESS_MS = 400L
        private const val TAP_MS = 250L
        private const val DOUBLE_TAP_MS = 300L
        private const val WHEEL_DELTA = 120f
    }
}
