package com.remoteview.client.input

import android.view.KeyEvent

// UNBUILT: chưa biên dịch.

data class ScanCode(val code: Int, val extended: Boolean = false)

/**
 * Ánh xạ sang scancode Set 1 (PS/2) mà SendInput KEYEVENTF_SCANCODE dùng.
 * KeyEvent.getScanCode() trả mã evdev của Linux: với khối phím chính (1..88) trùng Set 1,
 * các phím mở rộng (mũi tên, Home/End, Win, RCtrl...) cần tiền tố E0.
 */
object ScanCodeMap {
    val WIN = ScanCode(0x5B, true)
    val ESC = ScanCode(0x01)
    val TAB = ScanCode(0x0F)
    val ENTER = ScanCode(0x1C)
    val BACKSPACE = ScanCode(0x0E)
    val DELETE = ScanCode(0x53, true)
    val L_CTRL = ScanCode(0x1D)
    val L_ALT = ScanCode(0x38)
    val L_SHIFT = ScanCode(0x2A)
    val PRTSC = ScanCode(0x37, true)

    private val evdevExtended = mapOf(
        96 to ScanCode(0x1C, true), // KP Enter
        97 to ScanCode(0x1D, true), // Right Ctrl
        98 to ScanCode(0x35, true), // KP /
        99 to ScanCode(0x37, true), // SysRq / PrtSc
        100 to ScanCode(0x38, true), // Right Alt
        102 to ScanCode(0x47, true), // Home
        103 to ScanCode(0x48, true), // Up
        104 to ScanCode(0x49, true), // PgUp
        105 to ScanCode(0x4B, true), // Left
        106 to ScanCode(0x4D, true), // Right
        107 to ScanCode(0x4F, true), // End
        108 to ScanCode(0x50, true), // Down
        109 to ScanCode(0x51, true), // PgDn
        110 to ScanCode(0x52, true), // Insert
        111 to ScanCode(0x53, true), // Delete
        125 to ScanCode(0x5B, true), // Left Meta (Win)
        126 to ScanCode(0x5C, true), // Right Meta
        127 to ScanCode(0x5D, true), // Menu
    )

    fun fromLinux(evdev: Int): ScanCode? = when {
        evdev in 1..83 -> ScanCode(evdev)
        evdev == 87 -> ScanCode(0x57) // F11
        evdev == 88 -> ScanCode(0x58) // F12
        else -> evdevExtended[evdev]
    }

    private val androidKeys: Map<Int, ScanCode> = buildMap {
        val letters = mapOf(
            'Q' to 0x10, 'W' to 0x11, 'E' to 0x12, 'R' to 0x13, 'T' to 0x14, 'Y' to 0x15, 'U' to 0x16,
            'I' to 0x17, 'O' to 0x18, 'P' to 0x19, 'A' to 0x1E, 'S' to 0x1F, 'D' to 0x20, 'F' to 0x21,
            'G' to 0x22, 'H' to 0x23, 'J' to 0x24, 'K' to 0x25, 'L' to 0x26, 'Z' to 0x2C, 'X' to 0x2D,
            'C' to 0x2E, 'V' to 0x2F, 'B' to 0x30, 'N' to 0x31, 'M' to 0x32,
        )
        for ((ch, sc) in letters) put(KeyEvent.KEYCODE_A + (ch - 'A'), ScanCode(sc))
        // Số 1..9,0 -> 0x02..0x0B
        for (d in 0..9) put(KeyEvent.KEYCODE_0 + d, ScanCode(if (d == 0) 0x0B else 0x01 + d))
        put(KeyEvent.KEYCODE_MINUS, ScanCode(0x0C))
        put(KeyEvent.KEYCODE_EQUALS, ScanCode(0x0D))
        put(KeyEvent.KEYCODE_LEFT_BRACKET, ScanCode(0x1A))
        put(KeyEvent.KEYCODE_RIGHT_BRACKET, ScanCode(0x1B))
        put(KeyEvent.KEYCODE_SEMICOLON, ScanCode(0x27))
        put(KeyEvent.KEYCODE_APOSTROPHE, ScanCode(0x28))
        put(KeyEvent.KEYCODE_GRAVE, ScanCode(0x29))
        put(KeyEvent.KEYCODE_BACKSLASH, ScanCode(0x2B))
        put(KeyEvent.KEYCODE_COMMA, ScanCode(0x33))
        put(KeyEvent.KEYCODE_PERIOD, ScanCode(0x34))
        put(KeyEvent.KEYCODE_SLASH, ScanCode(0x35))
        put(KeyEvent.KEYCODE_SPACE, ScanCode(0x39))
        put(KeyEvent.KEYCODE_ENTER, ENTER)
        put(KeyEvent.KEYCODE_NUMPAD_ENTER, ScanCode(0x1C, true))
        put(KeyEvent.KEYCODE_TAB, TAB)
        put(KeyEvent.KEYCODE_DEL, BACKSPACE) // Android DEL = Backspace
        put(KeyEvent.KEYCODE_FORWARD_DEL, DELETE)
        put(KeyEvent.KEYCODE_ESCAPE, ESC)
        put(KeyEvent.KEYCODE_SHIFT_LEFT, L_SHIFT)
        put(KeyEvent.KEYCODE_SHIFT_RIGHT, ScanCode(0x36))
        put(KeyEvent.KEYCODE_CTRL_LEFT, L_CTRL)
        put(KeyEvent.KEYCODE_CTRL_RIGHT, ScanCode(0x1D, true))
        put(KeyEvent.KEYCODE_ALT_LEFT, L_ALT)
        put(KeyEvent.KEYCODE_ALT_RIGHT, ScanCode(0x38, true))
        put(KeyEvent.KEYCODE_META_LEFT, WIN)
        put(KeyEvent.KEYCODE_META_RIGHT, ScanCode(0x5C, true))
        put(KeyEvent.KEYCODE_CAPS_LOCK, ScanCode(0x3A))
        put(KeyEvent.KEYCODE_SYSRQ, PRTSC)
        put(KeyEvent.KEYCODE_DPAD_UP, ScanCode(0x48, true))
        put(KeyEvent.KEYCODE_DPAD_DOWN, ScanCode(0x50, true))
        put(KeyEvent.KEYCODE_DPAD_LEFT, ScanCode(0x4B, true))
        put(KeyEvent.KEYCODE_DPAD_RIGHT, ScanCode(0x4D, true))
        put(KeyEvent.KEYCODE_MOVE_HOME, ScanCode(0x47, true))
        put(KeyEvent.KEYCODE_MOVE_END, ScanCode(0x4F, true))
        put(KeyEvent.KEYCODE_PAGE_UP, ScanCode(0x49, true))
        put(KeyEvent.KEYCODE_PAGE_DOWN, ScanCode(0x51, true))
        put(KeyEvent.KEYCODE_INSERT, ScanCode(0x52, true))
        for (i in 0..9) put(KeyEvent.KEYCODE_F1 + i, ScanCode(0x3B + i))
        put(KeyEvent.KEYCODE_F11, ScanCode(0x57))
        put(KeyEvent.KEYCODE_F12, ScanCode(0x58))
    }

    fun fromAndroidKeyCode(keyCode: Int): ScanCode? = androidKeys[keyCode]

    /** Ưu tiên getScanCode() (bàn phím vật lý, qua One UI vẫn đọc được); rơi về keyCode. */
    fun fromKeyEvent(ev: KeyEvent): ScanCode? {
        if (ev.scanCode != 0) fromLinux(ev.scanCode)?.let { return it }
        return fromAndroidKeyCode(ev.keyCode)
    }
}
