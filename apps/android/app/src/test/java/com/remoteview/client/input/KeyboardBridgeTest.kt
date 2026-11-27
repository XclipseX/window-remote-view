package com.remoteview.client.input

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class KeyboardBridgeTest {

    private class HostBuffer(initial: String = "") {
        private val sb = StringBuilder(initial)

        fun apply(op: ImeDiffOp): ImeDiffOp {
            repeat(op.backspaces) {
                if (sb.isNotEmpty()) {
                    val cp = Character.codePointBefore(sb, sb.length)
                    sb.delete(sb.length - Character.charCount(cp), sb.length)
                }
            }
            if (op.insertText.isNotEmpty()) sb.append(op.insertText)
            return op
        }

        val text: String get() = sb.toString()
    }

    @Test
    fun vietnameseTelexViet() {
        val state = ImeDiffState()
        val host = HostBuffer()

        assertEquals(ImeDiffOp(0, 0, "v"), host.apply(state.setComposingText("v")))
        assertEquals(ImeDiffOp(0, 0, "i"), host.apply(state.setComposingText("vi")))
        assertEquals(ImeDiffOp(0, 0, "e"), host.apply(state.setComposingText("vie")))
        assertEquals(ImeDiffOp(0, 0, "t"), host.apply(state.setComposingText("viet")))
        assertEquals(ImeDiffOp(2, 0, "êt"), host.apply(state.setComposingText("viêt")))
        assertEquals(ImeDiffOp(2, 0, "ệt"), host.apply(state.setComposingText("việt")))
        assertEquals(ImeDiffOp(0, 0, " "), host.apply(state.commitText("việt ")))

        assertEquals("việt ", host.text)
        assertEquals("", state.composing)
        assertEquals("việt ", state.committedTail)
        assertEquals("việt ", state.getTextBeforeCursor(16))
    }

    @Test
    fun vietnameseTelexTuong() {
        val state = ImeDiffState()
        val host = HostBuffer()

        host.apply(state.setComposingText("t"))
        host.apply(state.setComposingText("tu"))
        host.apply(state.setComposingText("tuo"))
        host.apply(state.setComposingText("tuon"))
        host.apply(state.setComposingText("tuong"))
        assertEquals("tuong", host.text)

        assertEquals(ImeDiffOp(4, 0, "ương"), host.apply(state.setComposingText("tương")))
        assertEquals("tương", host.text)

        assertEquals(ImeDiffOp(3, 0, "ờng"), host.apply(state.setComposingText("tường")))
        assertEquals("tường", host.text)

        assertEquals(ImeDiffOp(0, 0, ""), host.apply(state.commitText("tường")))
        assertEquals("tường", host.text)
        assertEquals("", state.composing)
        assertEquals("tường", state.committedTail)
        assertEquals("tường", state.getTextBeforeCursor(10))
    }

    @Test
    fun emojiSurrogatePairDeleteInCodePoints() {
        val state = ImeDiffState()
        val host = HostBuffer()
        val grin = "\uD83D\uDE00"

        host.apply(state.commitText("hi "))
        host.apply(state.setComposingText(grin))
        assertEquals("hi $grin", host.text)

        // 1 code point = 2 UTF-16 chars: chỉ phát 1 Backspace và xóa sạch cặp surrogate khỏi composing.
        assertEquals(ImeDiffOp(1, 0, ""), host.apply(state.deleteSurroundingTextInCodePoints(1, 0)))
        assertEquals("hi ", host.text)
        assertEquals("", state.composing)
        assertEquals("hi ", state.committedTail)

        // commitText kế tiếp không phát thêm Backspace thừa.
        assertEquals(ImeDiffOp(0, 0, "!"), host.apply(state.commitText("!")))
        assertEquals("hi !", host.text)

        // Xóa 2 code point tràn vào committedTail.
        assertEquals(ImeDiffOp(2, 0, ""), host.apply(state.deleteSurroundingTextInCodePoints(2, 0)))
        assertEquals("hi", host.text)
        assertEquals("hi", state.committedTail)
    }

    @Test
    fun emojiSurrogatePairDiffAndDeleteSurroundingChars() {
        val state = ImeDiffState()
        val host = HostBuffer()
        val grin = "\uD83D\uDE00"
        val beam = "\uD83D\uDE01"

        host.apply(state.setComposingText(grin))
        // Hai emoji chung high surrogate '\uD83D': phải lùi về đầu cặp, xóa 1 code point và chèn nguyên emoji mới.
        assertEquals(ImeDiffOp(1, 0, beam), host.apply(state.setComposingText(beam)))
        assertEquals(beam, host.text)

        // deleteSurroundingText nhận độ dài UTF-16 (2 char cho 1 emoji) -> chỉ gửi 1 Backspace.
        assertEquals(ImeDiffOp(1, 0, ""), host.apply(state.deleteSurroundingText(2, 0)))
        assertEquals("", host.text)
        assertEquals("", state.composing)
    }

    @Test
    fun commitTextAfterDeleteSurroundingTextNoDoubleBackspace() {
        val state = ImeDiffState()
        val host = HostBuffer()

        host.apply(state.setComposingText("viet"))
        assertEquals(ImeDiffOp(1, 0, ""), host.apply(state.deleteSurroundingText(1, 0)))
        assertEquals("vie", host.text)
        assertEquals("vie", state.composing)

        // Commit lại phần còn lại không gây xóa kép.
        assertEquals(ImeDiffOp(0, 0, ""), host.apply(state.commitText("vie")))
        assertEquals("vie", host.text)
        assertEquals("", state.composing)
        assertEquals("vie", state.committedTail)

        // Sửa tiếp âm tiết mới sau khi xóa 1 ký tự cuối.
        host.apply(state.setComposingText("tuon"))
        assertEquals(ImeDiffOp(1, 0, ""), host.apply(state.deleteSurroundingText(1, 0)))
        assertEquals(ImeDiffOp(2, 0, "ươ"), host.apply(state.commitText("tươ")))
        assertEquals("vietươ", host.text)
        assertEquals("", state.composing)
        assertEquals("vietươ", state.committedTail)
    }

    @Test
    fun setComposingRegionReopensSyllableForToneAdjustment() {
        val state = ImeDiffState()
        val host = HostBuffer()

        host.apply(state.commitText("xin chao"))
        assertEquals("xin chao", state.getTextBeforeCursor(16))
        assertEquals("", state.composing)

        assertTrue(state.setComposingRegion(4, 8))
        assertEquals("xin ", state.committedTail)
        assertEquals("chao", state.composing)

        assertEquals(ImeDiffOp(2, 0, "ào"), host.apply(state.setComposingText("chào")))
        assertEquals("xin chào", host.text)

        assertEquals(ImeDiffOp(0, 0, " "), host.apply(state.commitText("chào ")))
        assertEquals("xin chào ", host.text)
        assertEquals("", state.composing)
        assertEquals("xin chào ", state.committedTail)

        // Vùng không kết thúc ở con trỏ bị từ chối an toàn, không làm lệch bộ đệm.
        assertFalse(state.setComposingRegion(0, 3))
        assertEquals("", state.composing)
    }

    @Test
    fun committedTailCapsAt64CharsWithoutSplittingSurrogates() {
        val grin = "\uD83D\uDE00"
        val state = ImeDiffState()
        state.commitText(grin + "b".repeat(63))
        assertEquals(63, state.committedTail.length)
        assertFalse(Character.isLowSurrogate(state.committedTail[0]))
        assertEquals("b".repeat(63), state.getTextBeforeCursor(64))

        val state2 = ImeDiffState()
        state2.commitText("x".repeat(66) + "chao")
        assertEquals(64, state2.committedTail.length)
        assertTrue(state2.committedTail.endsWith("chao"))

        // Hỗ trợ cả chỉ số tuyệt đối (66..70) lẫn chỉ số tương đối (60..64).
        assertTrue(state2.setComposingRegion(66, 70))
        assertEquals("chao", state2.composing)
        state2.finishComposingText()
        assertEquals("", state2.composing)

        assertTrue(state2.setComposingRegion(60, 64))
        assertEquals("chao", state2.composing)
    }

    @Test
    fun deleteBeyondLocalBufferAndBackspaceKey() {
        val empty = ImeDiffState()
        assertEquals(ImeDiffOp(2, 1, ""), empty.deleteSurroundingText(2, 1))
        assertEquals(ImeDiffOp(3, 2, ""), empty.deleteSurroundingTextInCodePoints(3, 2))

        val state = ImeDiffState()
        state.setComposingText("a\uD83D\uDE00")
        state.onBackspaceKey()
        assertEquals("a", state.composing)
        state.onBackspaceKey()
        assertEquals("", state.composing)
    }

    @Test
    fun finishComposingTextPreservesTail() {
        val state = ImeDiffState()
        state.setComposingText("viet")
        state.finishComposingText()
        assertEquals("", state.composing)
        assertEquals("viet", state.committedTail)
        assertEquals("viet", state.getTextBeforeCursor(10))
    }
}
