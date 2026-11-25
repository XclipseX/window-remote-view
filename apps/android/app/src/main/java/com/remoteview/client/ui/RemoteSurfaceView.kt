package com.remoteview.client.ui

import android.content.Context
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import com.remoteview.client.input.DirectTouchMapper
import com.remoteview.client.input.KeyboardBridge
import com.remoteview.client.input.StylusMapper
import com.remoteview.client.input.TouchGestureMapper

// UNBUILT: chưa biên dịch.

/** SurfaceView nhận mọi nguồn nhập liệu (cảm ứng, S Pen, phím vật lý, IME ảo). */
class RemoteSurfaceView(context: Context) : SurfaceView(context), SurfaceHolder.Callback {

    interface SurfaceListener {
        fun onSurfaceReady(holder: SurfaceHolder)
        fun onSurfaceDestroyed()
    }

    var surfaceListener: SurfaceListener? = null

    private var touch: TouchGestureMapper? = null
    private var stylus: StylusMapper? = null
    private var keyboard: KeyboardBridge? = null
    private var direct: DirectTouchMapper? = null
    private var zoom = 1f

    init {
        isFocusable = true
        isFocusableInTouchMode = true
        holder.addCallback(this)
    }

    fun bind(
        touch: TouchGestureMapper,
        stylus: StylusMapper,
        keyboard: KeyboardBridge,
        direct: DirectTouchMapper? = null,
    ) {
        this.direct = direct
        this.touch = touch
        this.stylus = stylus
        this.keyboard = keyboard
    }

    /** Zoom cục bộ (chụm/mở 2 ngón): chỉ phóng khung hình trên Android, không đổi gì phía Windows. */
    fun applyZoom(factor: Float, focusX: Float, focusY: Float) {
        zoom = (zoom * factor).coerceIn(1f, 4f)
        pivotX = focusX
        pivotY = focusY
        scaleX = zoom
        scaleY = zoom
    }

    fun toggleSoftKeyboard() {
        val imm = context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager
        requestFocus()
        imm.showSoftInput(this, InputMethodManager.SHOW_FORCED)
    }

    // ---- Surface
    override fun surfaceCreated(holder: SurfaceHolder) {}
    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
        surfaceListener?.onSurfaceReady(holder)
    }
    override fun surfaceDestroyed(holder: SurfaceHolder) {
        surfaceListener?.onSurfaceDestroyed()
    }

    // ---- Input
    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (stylus?.onTouch(this, event) == true) return true
        direct?.let { d -> if (d.onTouchEvent(this, event)) return true }
        if (direct == null && touch?.onTouchEvent(event, width, height) == true) return true
        return super.onTouchEvent(event)
    }

    override fun onHoverEvent(event: MotionEvent): Boolean {
        if (stylus?.onHover(this, event) == true) return true
        return super.onHoverEvent(event)
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean =
        keyboard?.onKeyEvent(event) == true || super.onKeyDown(keyCode, event)

    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean =
        keyboard?.onKeyEvent(event) == true || super.onKeyUp(keyCode, event)

    override fun onCheckIsTextEditor(): Boolean = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection? =
        keyboard?.createInputConnection(this, outAttrs)
}
