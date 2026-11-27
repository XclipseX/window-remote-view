package com.remoteview.client.input

import android.text.InputType
import android.view.KeyCharacterMap
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection

/**
 * Cầu nối bàn phím:
 *  - Phím vật lý (BT/OTG/dock) -> scancode qua KeyEvent.getScanCode() (đọc được cả phím One UI chặn),
 *    để UniKey/EVKey trên Windows tự xử lý Telex.
 *  - Bàn phím ảo -> [DiffInputConnection] gửi Unicode theo diff (đúng cả khi Windows tắt UniKey).
 *  - [rawTyping] = true: ký tự từ bàn phím ảo cũng được đổi thành phím (scancode) thay vì Unicode.
 */
class KeyboardBridge(internal val sink: InputSink) {

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
        outAttrs.initialSelStart = 0
        outAttrs.initialSelEnd = 0
        return DiffInputConnection(view, this)
    }
}

/** Kết quả diff cần phát sang host: số lần Backspace/Delete (theo code point) và chuỗi chèn mới. */
internal data class ImeDiffOp(
    val backspaces: Int = 0,
    val forwardDeletes: Int = 0,
    val insertText: String = "",
)

/**
 * Máy trạng thái thuần JVM cho bộ đệm IME (không phụ thuộc Android framework để unit-test trực tiếp):
 *  - Giữ [composing] và một đuôi [committedTail] (tối đa [maxTailChars] UTF-16 char) để
 *    `getTextBeforeCursor` trả về ngữ cảnh thật cho Samsung Keyboard / Gboard Telex.
 *  - Tính diff theo tiền tố chung trên đơn vị code point (an toàn với cặp surrogate UTF-16).
 *  - Hỗ trợ `setComposingRegion` mở lại âm tiết cuối trong đuôi khi IME sửa dấu tiếng Việt.
 */
internal class ImeDiffState(
    private val maxTailChars: Int = DEFAULT_MAX_TAIL_CHARS,
) {
    var composing: String = ""
        private set

    var committedTail: String = ""
        private set

    /** Số UTF-16 char đã bị cắt khỏi đầu [committedTail], dùng ánh xạ chỉ số tuyệt đối trong [setComposingRegion]. */
    private var baseOffset: Int = 0

    fun setComposingText(text: String): ImeDiffOp {
        val op = diffReplace(composing, text)
        composing = text
        return op
    }

    fun commitText(text: String): ImeDiffOp {
        val op = diffReplace(composing, text)
        composing = ""
        if (text.isNotEmpty()) {
            committedTail += text
            trimTail()
        }
        return op
    }

    fun finishComposingText() {
        if (composing.isEmpty()) return
        committedTail += composing
        composing = ""
        trimTail()
    }

    /**
     * Mở lại hậu tố kết thúc tại con trỏ thành [composing] để IME sửa dấu Telex trên âm tiết vừa commit.
     * Chấp nhận cả chỉ số tương đối theo bộ đệm hiện tại lẫn chỉ số tuyệt đối tích lũy từ đầu phiên.
     */
    fun setComposingRegion(start: Int, end: Int): Boolean {
        val a = minOf(start, end)
        val b = maxOf(start, end)
        if (a == b) {
            finishComposingText()
            return true
        }
        val full = committedTail + composing
        val localStart: Int
        val localEnd: Int
        when {
            b == full.length -> {
                localStart = a.coerceIn(0, full.length)
                localEnd = full.length
            }
            baseOffset > 0 && b == baseOffset + full.length -> {
                localStart = (a - baseOffset).coerceIn(0, full.length)
                localEnd = full.length
            }
            else -> {
                finishComposingText()
                return false
            }
        }
        if (localStart >= localEnd) {
            finishComposingText()
            return false
        }
        var cut = localStart
        if (cut > 0 && cut < full.length &&
            Character.isLowSurrogate(full[cut]) && Character.isHighSurrogate(full[cut - 1])
        ) {
            cut--
        }
        committedTail = full.substring(0, cut)
        composing = full.substring(cut)
        return true
    }

    /** Xóa theo độ dài UTF-16 char trước/sau con trỏ (theo hợp đồng `InputConnection.deleteSurroundingText`). */
    fun deleteSurroundingText(beforeChars: Int, afterChars: Int): ImeDiffOp {
        val reqBefore = beforeChars.coerceAtLeast(0)
        val reqAfter = afterChars.coerceAtLeast(0)
        val full = committedTail + composing
        val inBuf = minOf(reqBefore, full.length)
        val unknown = reqBefore - inBuf
        var cut = full.length - inBuf
        if (cut > 0 && cut < full.length &&
            Character.isLowSurrogate(full[cut]) && Character.isHighSurrogate(full[cut - 1])
        ) {
            cut--
        }
        val knownCp = if (cut < full.length) full.codePointCount(cut, full.length) else 0
        shrinkBufferByChars(full.length - cut)
        return ImeDiffOp(
            backspaces = knownCp + unknown,
            forwardDeletes = reqAfter,
        )
    }

    /** Xóa theo số Unicode code point trước/sau con trỏ (`InputConnection.deleteSurroundingTextInCodePoints`). */
    fun deleteSurroundingTextInCodePoints(beforeCp: Int, afterCp: Int): ImeDiffOp {
        val reqBefore = beforeCp.coerceAtLeast(0)
        val reqAfter = afterCp.coerceAtLeast(0)
        val full = committedTail + composing
        var idx = full.length
        var deletedCp = 0
        while (deletedCp < reqBefore && idx > 0) {
            val cp = Character.codePointBefore(full, idx)
            idx -= Character.charCount(cp)
            deletedCp++
        }
        val unknownCp = reqBefore - deletedCp
        shrinkBufferByChars(full.length - idx)
        return ImeDiffOp(
            backspaces = deletedCp + unknownCp,
            forwardDeletes = reqAfter,
        )
    }

    fun getTextBeforeCursor(n: Int): String {
        if (n <= 0) return ""
        val full = committedTail + composing
        if (n >= full.length) return full
        var start = full.length - n
        if (start > 0 && Character.isLowSurrogate(full[start]) && Character.isHighSurrogate(full[start - 1])) {
            start++
        }
        return full.substring(start)
    }

    /** Cập nhật bộ đệm cục bộ khi IME gửi phím `KEYCODE_DEL` trực tiếp (không phát thêm Backspace). */
    fun onBackspaceKey() {
        val full = committedTail + composing
        if (full.isEmpty()) return
        val cp = Character.codePointBefore(full, full.length)
        shrinkBufferByChars(Character.charCount(cp))
    }

    fun recordCommitted(text: String) {
        if (text.isEmpty()) return
        committedTail += text
        trimTail()
    }

    fun reset() {
        composing = ""
        committedTail = ""
        baseOffset = 0
    }

    private fun diffReplace(old: String, newText: String): ImeDiffOp {
        if (old == newText) return ImeDiffOp()
        val m = minOf(old.length, newText.length)
        var p = 0
        while (p < m && old[p] == newText[p]) p++
        // Đừng cắt giữa cặp surrogate UTF-16.
        if (p > 0 && Character.isHighSurrogate(old[p - 1])) p--
        val backspaces = if (p < old.length) old.codePointCount(p, old.length) else 0
        val insert = if (p < newText.length) newText.substring(p) else ""
        return ImeDiffOp(backspaces = backspaces, insertText = insert)
    }

    private fun shrinkBufferByChars(charsToRemove: Int) {
        if (charsToRemove <= 0) return
        if (charsToRemove <= composing.length) {
            composing = composing.substring(0, composing.length - charsToRemove)
        } else {
            val spill = charsToRemove - composing.length
            composing = ""
            committedTail = if (spill >= committedTail.length) {
                ""
            } else {
                committedTail.substring(0, committedTail.length - spill)
            }
        }
    }

    private fun trimTail() {
        val overflow = committedTail.length - maxTailChars
        if (overflow <= 0) return
        var cut = overflow
        if (cut < committedTail.length && Character.isLowSurrogate(committedTail[cut])) {
            cut++
        }
        committedTail = committedTail.substring(cut)
        baseOffset += cut
    }

    companion object {
        const val DEFAULT_MAX_TAIL_CHARS = 64
    }
}

/**
 * InputConnection ủy quyền tính diff và quản lý bộ đệm cho [ImeDiffState],
 * rồi phát phím Backspace/Delete (scancode) + văn bản Unicode qua [KeyboardBridge].
 */
internal class DiffInputConnection(
    view: View,
    private val bridge: KeyboardBridge,
    private val state: ImeDiffState = ImeDiffState(),
) : BaseInputConnection(view, false) {

    private fun applyOp(op: ImeDiffOp) {
        repeat(op.backspaces) { bridge.backspace() }
        repeat(op.forwardDeletes) { bridge.delete() }
        if (op.insertText.isNotEmpty()) bridge.emitText(op.insertText)
    }

    override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean {
        applyOp(state.setComposingText(text?.toString().orEmpty()))
        return true
    }

    override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
        applyOp(state.commitText(text?.toString().orEmpty()))
        return true
    }

    override fun finishComposingText(): Boolean {
        state.finishComposingText()
        return true
    }

    override fun setComposingRegion(start: Int, end: Int): Boolean {
        state.setComposingRegion(start, end)
        return true
    }

    override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
        applyOp(state.deleteSurroundingText(beforeLength, afterLength))
        return true
    }

    override fun deleteSurroundingTextInCodePoints(beforeLength: Int, afterLength: Int): Boolean {
        applyOp(state.deleteSurroundingTextInCodePoints(beforeLength, afterLength))
        return true
    }

    override fun sendKeyEvent(event: KeyEvent): Boolean {
        if (event.action == KeyEvent.ACTION_DOWN) {
            when (event.keyCode) {
                KeyEvent.KEYCODE_DEL -> state.onBackspaceKey()
                KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.KEYCODE_SHIFT_RIGHT,
                KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_CTRL_RIGHT,
                KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_ALT_RIGHT,
                KeyEvent.KEYCODE_META_LEFT, KeyEvent.KEYCODE_META_RIGHT,
                KeyEvent.KEYCODE_CAPS_LOCK -> Unit
                KeyEvent.KEYCODE_SPACE -> {
                    state.finishComposingText()
                    state.recordCommitted(" ")
                }
                else -> state.reset()
            }
        }
        return bridge.onKeyEvent(event) || super.sendKeyEvent(event)
    }

    override fun performEditorAction(editorAction: Int): Boolean {
        state.reset()
        bridge.enter()
        return true
    }

    override fun getTextBeforeCursor(n: Int, flags: Int): CharSequence = state.getTextBeforeCursor(n)
    override fun getTextAfterCursor(n: Int, flags: Int): CharSequence = ""
    override fun getSelectedText(flags: Int): CharSequence? = null
    override fun setSelection(start: Int, end: Int): Boolean = true
    override fun beginBatchEdit(): Boolean = true
    override fun endBatchEdit(): Boolean = true
}
