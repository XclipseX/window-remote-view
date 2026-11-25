package com.remoteview.client.ui

import android.app.Activity
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.remember
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
fun SessionScreen(controller: SessionController) {
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
        ModifierBar(
            keyboard = controller.keyboard,
            onToggleSoftKeyboard = { controller.toggleSoftKeyboard() },
            modifier = Modifier.align(Alignment.TopCenter),
        )
    }
    DisposableEffect(Unit) { onDispose { controller.release() } }
}

@Suppress("unused")
@Composable
fun rememberSessionController(settings: StreamSettings): SessionController {
    val activity = LocalContext.current as Activity
    return remember(settings) { SessionController(activity, settings) }
}
