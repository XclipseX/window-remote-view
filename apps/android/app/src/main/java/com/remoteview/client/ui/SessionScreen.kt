package com.remoteview.client.ui

import android.app.Activity
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.clickable
import android.widget.Toast
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.Canvas
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import kotlin.math.roundToInt
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.sp
import androidx.compose.ui.text.style.TextAlign
import android.os.SystemClock
import kotlinx.coroutines.delay
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.material3.MaterialTheme
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.remoteview.client.R
import androidx.compose.ui.platform.LocalView
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import android.os.Build
import android.view.WindowManager
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.viewinterop.AndroidView
import android.view.SurfaceHolder
import com.remoteview.client.StreamSettings
import com.remoteview.client.session.SessionController

// UNBUILT: chưa biên dịch.

/** Màn hình điều khiển: SurfaceView toàn màn + thanh phím nổi. */
@Composable
fun SessionScreen(controller: SessionController, showModifierBar: Boolean = true, debugEnabled: Boolean = true) {
    val usb = controller.usbDisplay
    val view = LocalView.current
    if (usb) {
        // Màn hình phụ: immersive toàn màn hình, tắt cạnh tai thỏ, luôn sáng.
        DisposableEffect(Unit) {
            val window = (view.context as? Activity)?.window
            if (window != null) {
                WindowCompat.setDecorFitsSystemWindows(window, false)
                if (Build.VERSION.SDK_INT >= 28) {
                    window.attributes = window.attributes.also {
                        it.layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
                    }
                }
                window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                val c = WindowInsetsControllerCompat(window, view)
                c.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
                c.hide(WindowInsetsCompat.Type.systemBars())
                view.keepScreenOn = true
                onDispose {
                    c.show(WindowInsetsCompat.Type.systemBars())
                    if (Build.VERSION.SDK_INT >= 28) {
                        window.attributes = window.attributes.also {
                            it.layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_DEFAULT
                        }
                    }
                }
            } else {
                onDispose { }
            }
        }
    }
    val streamSize by controller.streamSize.collectAsState()
    val aspect = if (streamSize.second > 0) streamSize.first.toFloat() / streamSize.second.toFloat() else 16f / 9f
    var zoomState by remember { mutableStateOf(Triple(1f, 0f, 0f)) }
    Box(Modifier.fillMaxSize().background(Color.Black)) {
        // Khung video đúng tỉ lệ luồng, căn giữa; phần còn lại đen (letterbox/pillarbox) và không nhận chạm.
        // Toàn bộ ánh xạ chạm/bút dùng kích thước của View này = content rect.
        Box(Modifier.align(Alignment.Center).aspectRatio(aspect)) {
            AndroidView(
                modifier = Modifier.fillMaxSize(),
                factory = { ctx ->
                    RemoteSurfaceView(ctx).also { v ->
                        controller.bindView(v)
                        v.onZoomChanged = { z, px, py -> zoomState = Triple(z, px, py) }
                        v.surfaceListener = object : RemoteSurfaceView.SurfaceListener {
                            override fun onSurfaceReady(holder: SurfaceHolder) {
                                controller.onSurfaceDestroyed() // tránh tạo trùng khi surfaceChanged lặp lại
                                controller.onSurfaceReady(holder.surface)
                            }
                            override fun onSurfaceDestroyed() = controller.onSurfaceDestroyed()
                        }
                        v.requestFocus()
                    }
                },
                onRelease = { controller.release() },
            )
            CursorOverlay(controller, zoomState, Modifier.fillMaxSize())
        }
        val reconnecting by controller.reconnecting.collectAsState()
        if (reconnecting) {
            // Không chặn cảm ứng: chỉ là nhãn nổi, phiên vẫn giữ.
            Row(
                modifier = Modifier
                    .align(Alignment.BottomCenter)
                    .padding(bottom = 32.dp)
                    .background(Color(0xE61C1B1F), RoundedCornerShape(28.dp))
                    .border(1.dp, Color(0x33FFFFFF), RoundedCornerShape(28.dp))
                    .padding(horizontal = 20.dp, vertical = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                CircularProgressIndicator(Modifier.size(20.dp), color = Color(0xFFFFB74D), strokeWidth = 2.dp)
                Text(
                    stringResource(R.string.session_reconnecting),
                    color = Color.White,
                    style = MaterialTheme.typography.bodyLarge,
                    modifier = Modifier.padding(start = 12.dp),
                )
            }
        }
        // --- Gỡ lỗi + chờ hình ---
        var tick by remember { mutableStateOf(0) }
        LaunchedEffect(Unit) { while (true) { tick++; delay(500) } }
        val snap = remember(tick) { controller.debugSnapshot() }
        val now = SystemClock.elapsedRealtime()
        val streaming = snap.framesDecoded > 0
        val waitingFrame = snap.establishedAtMs > 0 && !streaming && now - snap.establishedAtMs > 3000
        var manual by remember { mutableStateOf<Boolean?>(null) }
        val overlayVisible = manual ?: (debugEnabled && !streaming)
        if (waitingFrame) {
            Text(
                stringResource(R.string.session_waiting_frame),
                color = Color.White,
                textAlign = TextAlign.Center,
                modifier = Modifier
                    .align(Alignment.Center)
                    .background(Color(0xCC1C1B1F), RoundedCornerShape(24.dp))
                    .border(1.dp, Color(0x33FFFFFF), RoundedCornerShape(24.dp))
                    .padding(horizontal = 24.dp, vertical = 16.dp),
            )
        }
        if (overlayVisible) {
            val ctx = LocalContext.current
            val okMsg = stringResource(R.string.log_upload_ok)
            val failMsg = stringResource(R.string.log_upload_fail)
            val needMsg = stringResource(R.string.log_upload_need_session)
            Column(
                modifier = Modifier
                    .align(Alignment.TopStart)
                    .padding(top = 8.dp, start = 8.dp)
                    .background(Color(0xB31C1B1F), RoundedCornerShape(16.dp))
                    .border(1.dp, Color(0x33FFFFFF), RoundedCornerShape(16.dp))
                    .padding(12.dp),
            ) {
                Text(
                    debugText(snap, now),
                    color = Color(0xFF9CFF9C),
                    fontFamily = FontFamily.Monospace,
                    fontSize = 11.sp,
                    lineHeight = 13.sp,
                )
                Text(
                    stringResource(R.string.log_upload),
                    color = Color(0xFFFFB74D),
                    fontSize = 13.sp,
                    modifier = Modifier
                        .padding(top = 8.dp)
                        .clickable {
                            val h = if (controller.isConnected) controller.nativeHandle else 0L
                            if (h == 0L) {
                                Toast.makeText(ctx, needMsg, Toast.LENGTH_SHORT).show()
                            } else {
                                Thread({
                                    val ok = com.remoteview.client.LogShare.upload(h)
                                    android.os.Handler(android.os.Looper.getMainLooper()).post {
                                        Toast.makeText(ctx, if (ok) okMsg else failMsg, Toast.LENGTH_SHORT).show()
                                    }
                                }, "rv-log-upload").start()
                            }
                        }
                        .padding(vertical = 6.dp),
                )
            }
        }
        // Nhấn giữ góc trên-phải để bật/tắt lớp phủ (góc này không chuyển cảm ứng cho host).
        Box(
            Modifier
                .align(Alignment.TopEnd)
                .size(64.dp)
                .pointerInput(overlayVisible) {
                    detectTapGestures(onLongPress = { manual = !overlayVisible })
                },
        )
        if (showModifierBar) {
            ModifierBar(
                keyboard = controller.keyboard,
                onToggleSoftKeyboard = { controller.toggleSoftKeyboard() },
                modifier = Modifier.align(Alignment.TopCenter),
            )
        }
    }
    DisposableEffect(Unit) { onDispose { controller.release() } }
}

@Suppress("unused")
@Composable
fun rememberSessionController(settings: StreamSettings): SessionController {
    val activity = LocalContext.current as Activity
    return remember(settings) { SessionController(activity, settings) }
}

private fun debugText(d: com.remoteview.client.session.DebugSnapshot, now: Long): String {
    val est = if (d.establishedAtMs > 0) "${(now - d.establishedAtMs) / 1000}s" else "chưa"
    return buildString {
        appendLine("link: ${d.linkType} mở=${if (d.linkOpen) "có" else "không"}  AOA fd: ${d.aoaOpened}")
        appendLine("byte vào/ra: ${d.bytesIn} / ${d.bytesOut}")
        appendLine("datagram vào/ra: ${d.datagramsIn} / ${d.datagramsOut}")
        appendLine("bắt tay: ${d.handshake} (handle=${d.nativeHandle}, established: $est)")
        appendLine("native: state=${d.nativeState} \"${d.nativeMessage}\"")
        appendLine("decoder: ${d.decoderName}")
        appendLine("frame nhận/giải mã: ${d.framesSubmitted} / ${d.framesDecoded}")
        append("lỗi cuối: ${d.lastError.ifEmpty { "-" }}")
    }
}

/** Con trỏ chuột của host vẽ phủ lên video: vị trí 0..65535 trên khung hình, trừ điểm nóng, nhân tỉ lệ content rect. */
@Composable
private fun CursorOverlay(controller: SessionController, zoom: Triple<Float, Float, Float>, modifier: Modifier) {
    val c by controller.cursor.collectAsState()
    val streamSize by controller.streamSize.collectAsState()
    val img = remember(c.bitmap) { c.bitmap?.asImageBitmap() }
    Canvas(
        modifier.graphicsLayer {
            scaleX = zoom.first
            scaleY = zoom.first
            transformOrigin = if (size.width > 0f && size.height > 0f) {
                TransformOrigin(zoom.second / size.width, zoom.third / size.height)
            } else TransformOrigin.Center
        },
    ) {
        if (!c.visible || size.width <= 0f || size.height <= 0f) return@Canvas
        val sw = streamSize.first.coerceAtLeast(1)
        val scale = size.width / sw
        val px = c.x / 65535f * size.width
        val py = c.y / 65535f * size.height
        if (img != null) {
            drawImage(
                img,
                srcOffset = IntOffset.Zero,
                srcSize = IntSize(img.width, img.height),
                dstOffset = IntOffset((px - c.hotX * scale).roundToInt(), (py - c.hotY * scale).roundToInt()),
                dstSize = IntSize((img.width * scale).roundToInt().coerceAtLeast(1), (img.height * scale).roundToInt().coerceAtLeast(1)),
            )
        } else {
            // Mũi tên mặc định (điểm nóng ở đỉnh), cao ~20 pixel host.
            val k = scale
            val path = Path().apply {
                moveTo(px, py)
                lineTo(px, py + 17f * k)
                lineTo(px + 4f * k, py + 13f * k)
                lineTo(px + 7f * k, py + 19f * k)
                lineTo(px + 9.5f * k, py + 18f * k)
                lineTo(px + 6.5f * k, py + 12f * k)
                lineTo(px + 12f * k, py + 12f * k)
                close()
            }
            drawPath(path, Color.White)
            drawPath(path, Color.Black, style = Stroke(width = (1.2f * k).coerceAtLeast(1f)))
        }
    }
}
