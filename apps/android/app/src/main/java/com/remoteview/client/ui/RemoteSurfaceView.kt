package com.remoteview.client.ui

import android.content.Context
import android.os.Build
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.WindowInsets
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import com.remoteview.client.input.DirectTouchMapper
import com.remoteview.client.input.InputSink
import com.remoteview.client.input.KeyboardBridge
import com.remoteview.client.input.MouseButtonId
import com.remoteview.client.input.RemoteInput
import com.remoteview.client.input.StylusMapper
import com.remoteview.client.input.TouchGestureMapper
import kotlin.math.roundToInt

/** SurfaceView nhận mọi nguồn nhập liệu (cảm ứng, S Pen, chuột ngoài, phím vật lý, IME ảo). */
class RemoteSurfaceView(context: Context) : SurfaceView(context), SurfaceHolder.Callback {

    interface SurfaceListener {
        fun onSurfaceReady(holder: SurfaceHolder)
        fun onSurfaceDestroyed()
    }

    var surfaceListener: SurfaceListener? = null

    /** (zoom, equivPivotX, equivPivotY) mỗi khi zoom/pan đổi, để lớp phủ con trỏ phóng khớp tuyệt đối. */
    var onZoomChanged: ((Float, Float, Float) -> Unit)? = null

    private var touch: TouchGestureMapper? = null
    private var stylus: StylusMapper? = null
    private var keyboard: KeyboardBridge? = null
    private var direct: DirectTouchMapper? = null
    private var inputSink: InputSink? = null

    private var zoom = 1f
    private var panX = 0f
    private var panY = 0f
    private var imeVisibleFallback = false
    private var mouseButtonsDown = 0

    init {
        isFocusable = true
        isFocusableInTouchMode = true
        pivotX = 0f
        pivotY = 0f
        holder.addCallback(this)
    }

    fun bind(
        touch: TouchGestureMapper,
        stylus: StylusMapper,
        keyboard: KeyboardBridge,
        direct: DirectTouchMapper? = null,
        sink: InputSink? = null,
    ) {
        this.direct = direct
        this.touch = touch
        this.stylus = stylus
        this.keyboard = keyboard
        this.inputSink = sink ?: keyboard.sink
        touch.onPinchZoomPan = { factor, fx, fy, dx, dy -> applyZoomPan(factor, fx, fy, dx, dy) }
    }

    /** Zoom cục bộ (chụm/mở 2 ngón): chỉ phóng khung hình trên Android, không đổi gì phía Windows. */
    fun applyZoom(factor: Float, focusX: Float, focusY: Float) {
        applyZoomPan(factor, focusX, focusY, 0f, 0f)
    }

    /**
     * Zoom + pan cục bộ với pivot cố định (0, 0) và translation bị kẹp trong biên [-(s - 1) * W..0],
     * tránh vòng phản hồi biến đổi ngược MotionEvent gây giật/dao động.
     */
    fun applyZoomPan(
        factor: Float,
        focusViewX: Float,
        focusViewY: Float,
        panDx: Float = 0f,
        panDy: Float = 0f,
    ) {
        if (!factor.isFinite() || factor <= 0f) return
        val oldZoom = zoom
        val newZoom = (oldZoom * factor).coerceIn(1f, MAX_ZOOM)
        pivotX = 0f
        pivotY = 0f
        if (newZoom <= RESET_ZOOM_THRESHOLD) {
            zoom = 1f
            panX = 0f
            panY = 0f
        } else {
            val actualFactor = newZoom / oldZoom
            val rawTx = focusViewX - (focusViewX - panDx - panX) * actualFactor
            val rawTy = focusViewY - (focusViewY - panDy - panY) * actualFactor
            val minTx = -((newZoom - 1f) * width).coerceAtLeast(0f)
            val minTy = -((newZoom - 1f) * height).coerceAtLeast(0f)
            zoom = newZoom
            panX = rawTx.coerceIn(minTx, 0f)
            panY = rawTy.coerceIn(minTy, 0f)
        }
        applyTransformAndNotify()
    }

    private fun applyTransformAndNotify() {
        pivotX = 0f
        pivotY = 0f
        scaleX = zoom
        scaleY = zoom
        translationX = panX
        translationY = panY
        // Quy đổi (scale, translation) với pivot=(0,0) sang pivot tương đương để CursorOverlay khớp chính xác.
        val equivPivotX = if (zoom > 1.0001f) -panX / (zoom - 1f) else 0f
        val equivPivotY = if (zoom > 1.0001f) -panY / (zoom - 1f) else 0f
        onZoomChanged?.invoke(zoom, equivPivotX, equivPivotY)
    }

    fun toggleSoftKeyboard() {
        val imm = context.getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager ?: return
        val isVisible = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            rootWindowInsets?.isVisible(WindowInsets.Type.ime()) ?: imeVisibleFallback
        } else {
            imeVisibleFallback
        }
        if (isVisible) {
            windowToken?.let { imm.hideSoftInputFromWindow(it, 0) }
            imeVisibleFallback = false
        } else {
            requestFocus()
            imeVisibleFallback = imm.showSoftInput(this, InputMethodManager.SHOW_IMPLICIT)
        }
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        if (zoom > RESET_ZOOM_THRESHOLD && w > 0 && h > 0) {
            val minTx = -((zoom - 1f) * w).coerceAtLeast(0f)
            val minTy = -((zoom - 1f) * h).coerceAtLeast(0f)
            panX = panX.coerceIn(minTx, 0f)
            panY = panY.coerceIn(minTy, 0f)
            applyTransformAndNotify()
        } else if (zoom > 1f) {
            zoom = 1f
            panX = 0f
            panY = 0f
            applyTransformAndNotify()
        }
    }

    override fun onDetachedFromWindow() {
        inputSink?.let { syncMouseButtons(0, it) }
        super.onDetachedFromWindow()
    }

    // ---- Surface
    private var pendingReady = false
    override fun surfaceCreated(holder: SurfaceHolder) { pendingReady = true }
    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
        // Đổi kích thước (đổi tỉ lệ luồng, xoay) cùng một Surface thì không dựng lại decoder.
        if (pendingReady) {
            pendingReady = false
            surfaceListener?.onSurfaceReady(holder)
        }
    }
    override fun surfaceDestroyed(holder: SurfaceHolder) {
        pendingReady = false
        surfaceListener?.onSurfaceDestroyed()
    }

    // ---- Input
    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (isExternalMouse(event)) {
            return handleMouseTouchEvent(event)
        }
        if (stylus?.onTouch(this, event) == true) return true
        direct?.let { d -> if (d.onTouchEvent(this, event)) return true }
        if (direct == null && touch?.onTouchEvent(event, width, height, scaleX, translationX, translationY) == true) {
            return true
        }
        return super.onTouchEvent(event)
    }

    override fun onHoverEvent(event: MotionEvent): Boolean {
        if (isExternalMouse(event)) {
            val s = inputSink
            if (s != null && event.actionMasked == MotionEvent.ACTION_HOVER_MOVE) {
                emitMouseMoveAbs(event, s)
                return true
            }
        }
        if (stylus?.onHover(this, event) == true) return true
        return super.onHoverEvent(event)
    }

    override fun onGenericMotionEvent(event: MotionEvent): Boolean {
        if (isExternalMouse(event)) {
            val s = inputSink
            if (s != null) {
                when (event.actionMasked) {
                    MotionEvent.ACTION_HOVER_MOVE, MotionEvent.ACTION_MOVE -> {
                        emitMouseMoveAbs(event, s)
                        return true
                    }
                    MotionEvent.ACTION_SCROLL -> {
                        val hScroll = (event.getAxisValue(MotionEvent.AXIS_HSCROLL) * WHEEL_DELTA).roundToInt()
                        val vScroll = (event.getAxisValue(MotionEvent.AXIS_VSCROLL) * WHEEL_DELTA).roundToInt()
                        if (hScroll != 0 || vScroll != 0) {
                            s.send(RemoteInput.MouseWheel(horizontal = hScroll, vertical = vScroll))
                        }
                        return true
                    }
                    MotionEvent.ACTION_BUTTON_PRESS -> {
                        emitMouseMoveAbs(event, s)
                        syncMouseButtons(event.buttonState or event.actionButton, s)
                        return true
                    }
                    MotionEvent.ACTION_BUTTON_RELEASE -> {
                        emitMouseMoveAbs(event, s)
                        syncMouseButtons(event.buttonState and event.actionButton.inv(), s)
                        return true
                    }
                }
            }
        }
        return super.onGenericMotionEvent(event)
    }

    private fun isExternalMouse(event: MotionEvent): Boolean =
        event.isFromSource(InputDevice.SOURCE_MOUSE) && !hasPointerCapture()

    private fun handleMouseTouchEvent(event: MotionEvent): Boolean {
        val s = inputSink ?: return false
        return when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                emitMouseMoveAbs(event, s)
                val targetButtons = if (event.buttonState != 0) event.buttonState else MotionEvent.BUTTON_PRIMARY
                syncMouseButtons(targetButtons, s)
                true
            }
            MotionEvent.ACTION_MOVE -> {
                emitMouseMoveAbs(event, s)
                if (event.buttonState != 0) {
                    syncMouseButtons(event.buttonState, s)
                }
                true
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                if (event.actionMasked == MotionEvent.ACTION_UP) {
                    emitMouseMoveAbs(event, s)
                }
                syncMouseButtons(0, s)
                true
            }
            MotionEvent.ACTION_BUTTON_PRESS -> {
                emitMouseMoveAbs(event, s)
                syncMouseButtons(event.buttonState or event.actionButton, s)
                true
            }
            MotionEvent.ACTION_BUTTON_RELEASE -> {
                emitMouseMoveAbs(event, s)
                syncMouseButtons(event.buttonState and event.actionButton.inv(), s)
                true
            }
            else -> false
        }
    }

    private fun emitMouseMoveAbs(event: MotionEvent, sink: InputSink) {
        val w = width
        val h = height
        if (w <= 0 || h <= 0) return
        val nx = (event.x / w).coerceIn(0f, 1f)
        val ny = (event.y / h).coerceIn(0f, 1f)
        sink.send(RemoteInput.MouseMoveAbs(nx, ny))
    }

    private fun syncMouseButtons(newState: Int, sink: InputSink) {
        val changed = mouseButtonsDown xor newState
        if (changed == 0) return
        for ((flag, id) in MOUSE_BUTTON_MAP) {
            if ((changed and flag) != 0) {
                sink.send(RemoteInput.MouseButton(id, (newState and flag) != 0))
            }
        }
        mouseButtonsDown = newState
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean =
        keyboard?.onKeyEvent(event) == true || super.onKeyDown(keyCode, event)

    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean =
        keyboard?.onKeyEvent(event) == true || super.onKeyUp(keyCode, event)

    override fun onCheckIsTextEditor(): Boolean = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection? =
        keyboard?.createInputConnection(this, outAttrs)

    private companion object {
        const val MAX_ZOOM = 4f
        const val RESET_ZOOM_THRESHOLD = 1.01f
        const val WHEEL_DELTA = 120f

        val MOUSE_BUTTON_MAP = arrayOf(
            MotionEvent.BUTTON_PRIMARY to MouseButtonId.LEFT,
            MotionEvent.BUTTON_SECONDARY to MouseButtonId.RIGHT,
            MotionEvent.BUTTON_TERTIARY to MouseButtonId.MIDDLE,
            MotionEvent.BUTTON_BACK to MouseButtonId.X1,
            MotionEvent.BUTTON_FORWARD to MouseButtonId.X2,
        )
    }
}
