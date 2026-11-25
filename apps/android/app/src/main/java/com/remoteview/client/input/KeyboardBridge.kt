package com.remoteview.client.input

import android.os.SystemClock
import android.text.InputType
import android.view.KeyCharacterMap
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection

// UNBUILT: chưa biên dịch.

/**
 * Cầu nối bàn phím:
 *  - Phím vật lý (BT/OTG/dock) -> scancode qua KeyEvent.getScanCode() (đọc được cả phím One UI chặn),
 *    để UniKey/EVKey trên Windows tự xử lý Telex.
 *  - Bàn phím ảo -> [DiffInputConnection] gửi Unicode theo diff (đúng cả khi Windows tắt UniKey).
 *  - [rawTyping] = true: ký tự từ bàn phím ảo cũng được đổi thành phím (scancode) thay vì Unicode.
 */
class KeyboardBridge(private val sink: InputSink) {

    @Volatile var rawTyping: Boolean = false

    // ---------------------------------------------------------------- phím vật lý / ảo dạng KeyEvent

    /** @return true nếu đã tiêu thụ sự kiện (đã gửi sang host). */
    fun onKeyEvent(ev: KeyEvent): Boolean {
        if (ev.keyCode == KeyEvent.KEYCODE_BACK) return false // để hệ thống xử lý Back
        val sc = ScanCodeMap.fromKeyEvent(ev) ?: return false
        when (ev.action) {
            KeyEvent.ACTION_DOWN -> sink.send(RemoteInput.Key(sc.code, sc.extended, true))
            KeyEvent.ACTION_UP -> sink.send(RemoteInput.Key(sc.code, sc.extended, false))
            else -> return false
        }
        return true
    }

    fun tap(sc: ScanCode) {
        sink.send(RemoteInput.Key(sc.code, sc.extended, true))
        sink.send(RemoteInput.Key(sc.code, sc.extended, false))
    }

    /** Nhấn giữ các modifier, bấm [key], rồi nhả ngược thứ tự. Ví dụ Alt+Tab. */
    fun chord(modifiers: List<ScanCode>, key: ScanCode) {
        for (m in modifiers) sink.send(RemoteInput.Key(m.code, m.extended, true))
        tap(key)
        for (m in modifiers.asReversed()) sink.send(RemoteInput.Key(m.code, m.extended, false))
    }

    fun backspace() = tap(ScanCodeMap.BACKSPACE)
    fun delete() = tap(ScanCodeMap.DELETE)
    fun enter() = tap(ScanCodeMap.ENTER)

    fun sendSecureAttention() = sink.send(RemoteInput.SecureAttention)

    // ---------------------------------------------------------------- văn bản từ IME

    /** Gửi văn bản: Unicode (mặc định) hoặc chuỗi phím nếu [rawTyping]. */
    fun emitText(s: String) {
        if (s.isEmpty()) return
        if (rawTyping) {
            val events = KeyCharacterMap.load(KeyCharacterMap.VIRTUAL_KEYBOARD).getEvents(s.toCharArray())
            if (events != null && events.all { ScanCodeMap.fromAndroidKeyCode(it.keyCode) != null }) {
                events.forEach { onKeyEvent(it) }
                return
            }
        }
        sink.send(RemoteInput.Text(s))
    }

    fun createInputConnection(view: View, outAttrs: EditorInfo): InputConnection {
        outAttrs.inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS or
            (if (rawTyping) InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD else 0)
        outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN or EditorInfo.IME_FLAG_NO_EXTRACT_UI or
            EditorInfo.IME_ACTION_NONE
        return DiffInputConnection(view, this)
    }
}

/**
 * InputConnection không giữ văn bản thật: chỉ nhớ đoạn đang "composing" đã gửi sang host và,
 * mỗi lần IME đổi đoạn đó (Telex: "viet" -> "việt"), gửi Backspace cho phần khác + Unicode cho phần mới.
 *
 * Giới hạn đã biết: setComposingRegion() trên văn bản đã commit không thể dựng lại (không có văn bản),
 * nên bị bỏ qua. TODO: giữ bộ đệm từ vừa commit nếu Gboard/Laban Key cần.
 */
internal class DiffInputConnection(
    view: View,
    private val bridge: KeyboardBridge,
) : BaseInputConnection(view, false) {

    private var composing = ""

    private fun replaceComposing(newText: String) {
        val old = composing
        val m = minOf(old.length, newText.length)
        var p = 0
        while (p < m && old[p] == newText[p]) p++
        if (p > 0 && p < old.length && Character.isHighSurrogate(old[p - 1])) p-- // đừng cắt giữa cặp thay thế
        repeat(old.codePointCount(p, old.length)) { bridge.backspace() }
        if (p < newText.length) bridge.emitText(newText.substring(p))
        composing = newText
    }

    override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean {
        replaceComposing(text?.toString().orEmpty())
        return true
    }

    override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
        replaceComposing(text?.toString().orEmpty())
        composing = ""
        return true
    }

    override fun finishComposingText(): Boolean {
        composing = ""
        return true
    }

    override fun setComposingRegion(start: Int, end: Int): Boolean = true

    override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
        repeat(beforeLength) { bridge.backspace() }
        repeat(afterLength) { bridge.delete() }
        composing = composing.dropLast(beforeLength)
        return true
    }

    override fun deleteSurroundingTextInCodePoints(beforeLength: Int, afterLength: Int): Boolean =
        deleteSurroundingText(beforeLength, afterLength)

    override fun sendKeyEvent(event: KeyEvent): Boolean {
        if (event.keyCode == KeyEvent.KEYCODE_DEL && event.action == KeyEvent.ACTION_DOWN) {
            composing = composing.dropLast(1)
        } else if (composing.isNotEmpty() && event.action == KeyEvent.ACTION_DOWN &&
            event.keyCode != KeyEvent.KEYCODE_SHIFT_LEFT && event.keyCode != KeyEvent.KEYCODE_SHIFT_RIGHT
        ) {
            composing = ""
        }
        return bridge.onKeyEvent(event) || super.sendKeyEvent(event)
    }

    override fun performEditorAction(editorAction: Int): Boolean {
        composing = ""
        bridge.enter()
        return true
    }

    // IME đọc ngữ cảnh: chỉ cung cấp đoạn composing để nó không đoán sai.
    override fun getTextBeforeCursor(n: Int, flags: Int): CharSequence = composing.takeLast(n)
    override fun getTextAfterCursor(n: Int, flags: Int): CharSequence = ""
    override fun getSelectedText(flags: Int): CharSequence? = null
    override fun setSelection(start: Int, end: Int): Boolean = true
    override fun beginBatchEdit(): Boolean = true
    override fun endBatchEdit(): Boolean = true

    @Suppress("unused")
    private fun now() = SystemClock.uptimeMillis()
}
