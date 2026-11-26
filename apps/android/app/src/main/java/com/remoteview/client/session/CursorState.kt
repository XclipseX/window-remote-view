package com.remoteview.client.session

import android.graphics.Bitmap

/** Trạng thái con trỏ host: hình (null = dùng mũi tên mặc định), điểm nóng, vị trí 0..65535 trên khung hình. */
data class CursorState(
    val bitmap: Bitmap? = null,
    val hotX: Int = 0,
    val hotY: Int = 0,
    val x: Int = 0,
    val y: Int = 0,
    val visible: Boolean = false,
)
