package com.remoteview.client.ui

import android.app.Activity
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.foundation.background
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
fun SessionScreen(controller: SessionController, showModifierBar: Boolean = true) {
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
    Box(Modifier.fillMaxSize()) {
        AndroidView(
            modifier = Modifier.fillMaxSize(),
            factory = { ctx ->
                RemoteSurfaceView(ctx).also { v ->
                    controller.bindView(v)
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
        val reconnecting by controller.reconnecting.collectAsState()
        if (reconnecting) {
            // Không chặn cảm ứng: chỉ là nhãn nổi, phiên vẫn giữ.
            Row(
                modifier = Modifier
                    .align(Alignment.BottomCenter)
                    .padding(bottom = 24.dp)
                    .background(Color(0xCC000000), RoundedCornerShape(24.dp))
                    .padding(horizontal = 16.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                CircularProgressIndicator(Modifier.size(18.dp), color = Color.White, strokeWidth = 2.dp)
                Text(
                    stringResource(R.string.session_reconnecting),
                    color = Color.White,
                    modifier = Modifier.padding(start = 12.dp),
                )
            }
        }
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
