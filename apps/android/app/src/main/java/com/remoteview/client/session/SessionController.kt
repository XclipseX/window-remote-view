package com.remoteview.client.session

import android.app.Activity
import android.content.Context
import android.os.Process
import android.os.SystemClock
import android.util.DisplayMetrics
import android.view.Surface
import com.remoteview.client.NativeBridge
import com.remoteview.client.NativeInputSink
import com.remoteview.client.StreamSettings
import com.remoteview.client.decoder.AnnexB
import com.remoteview.client.decoder.CodecPreference
import com.remoteview.client.decoder.DecoderPolicy
import com.remoteview.client.decoder.EncodedFrame
import com.remoteview.client.decoder.LowLatencyDecoder
import com.remoteview.client.decoder.StallWatchdog
import com.remoteview.client.input.KeyboardBridge
import com.remoteview.client.input.PointerCaptureController
import com.remoteview.client.input.StylusMapper
import com.remoteview.client.input.TouchGestureMapper
import com.remoteview.client.perf.AdpfSession
import com.remoteview.client.perf.ThermalGovernor
import com.remoteview.client.render.DisplayLock
import com.remoteview.client.render.FramePresenter
import com.remoteview.client.ui.RemoteSurfaceView

// UNBUILT: chưa biên dịch.

/** Nối decoder + presenter + watchdog + ADPF + thermal + input với nhau cho một phiên. */
class SessionController(
    private val activity: Activity,
    private val settings: StreamSettings,
) {
    @Volatile var nativeHandle: Long = 0L

    private val sink = NativeInputSink { nativeHandle }
    val keyboard = KeyboardBridge(sink).also { it.rawTyping = settings.rawTyping }
    private val stylus = StylusMapper(sink)
    private val touch = TouchGestureMapper(activity, sink, settings.touchMode) { s, fx, fy ->
        view?.applyZoom(s, fx, fy)
    }

    private var view: RemoteSurfaceView? = null
    private var capture: PointerCaptureController? = null
    private var decoder: LowLatencyDecoder? = null
    private var presenter: FramePresenter? = null
    private var watchdog: StallWatchdog? = null
    private var adpf: AdpfSession? = null
    private var thermal: ThermalGovernor? = null
    private var surface: Surface? = null
    private var effectiveFps = settings.fps
    private var lastFrameSubmitMs = 0L

    fun bindView(v: RemoteSurfaceView) {
        view = v
        v.bind(touch, stylus, keyboard)
        capture = PointerCaptureController(v, sink).also { it.enable() }
    }

    fun toggleSoftKeyboard() { view?.toggleSoftKeyboard() }

    fun onWindowFocusChanged(hasFocus: Boolean) = capture?.onWindowFocusChanged(hasFocus)

    fun onSurfaceReady(s: Surface) {
        surface = s
        DisplayLock.lockHighestRefresh(activity)
        DisplayLock.applyFrameRate(s, effectiveFps.toFloat())

        val (w, h) = decodeSize()
        val keyFrame = { if (NativeBridge.available && nativeHandle != 0L) NativeBridge.nativeRequestKeyFrame(nativeHandle) }
        val dec = LowLatencyDecoder(
            w, h, effectiveFps,
            DecoderPolicy(settings.codec, settings.allowAv1, settings.hardwareDecode),
            object : LowLatencyDecoder.Listener {
                override fun onKeyFrameNeeded() = keyFrame()
                override fun onFatal(reason: String) {
                    // TODO: báo host hạ về H.264 / báo UI.
                }
            },
        )
        val pres = FramePresenter(
            dec,
            if (settings.directPresent) FramePresenter.Mode.DIRECT else FramePresenter.Mode.CHOREOGRAPHER,
        ) { idle -> surface?.let { DisplayLock.setIdle(it, idle, effectiveFps.toFloat()) } }
        dec.onOutput = pres::onOutput
        decoder = dec
        presenter = pres

        if (dec.start(s)) {
            watchdog = StallWatchdog(object : StallWatchdog.Host {
                override val lastInputMs get() = dec.lastInputMs
                override val lastOutputMs get() = dec.lastOutputMs
                override fun flushDecoder() { pres.clear(); dec.flush() }
                override fun recreateDecoder() { pres.clear(); dec.recreate(escalate = true) }
                override fun requestKeyFrame() = keyFrame()
            }).also { it.start() }
        }

        adpf = AdpfSession(activity, intArrayOf(Process.myTid()), effectiveFps)
        thermal = ThermalGovernor(activity, object : ThermalGovernor.Listener {
            override fun onBitrateScale(scale: Float) {
                if (NativeBridge.available && nativeHandle != 0L) {
                    NativeBridge.nativeSetBitrate(nativeHandle, (settings.targetBitrateKbps() * scale).toInt())
                }
            }

            override fun onFpsCap(maxFps: Int?) {
                effectiveFps = minOf(settings.fps, maxFps ?: settings.fps)
                surface?.let { DisplayLock.applyFrameRate(it, effectiveFps.toFloat()) }
                adpf?.updateTarget(effectiveFps)
                if (NativeBridge.available && nativeHandle != 0L) NativeBridge.nativeSetMaxFps(nativeHandle, effectiveFps)
            }
        }).also { it.start() }

        NativeBridge.callbacks = object : NativeBridge.Callbacks {
            override fun onVideoFrame(data: ByteArray, ptsUs: Long, flags: Int) = submitFrame(data, ptsUs, flags)
            override fun onConnectionState(state: Int, message: String?) {}
            override fun onHostDiscovered(name: String, address: String) {}
            override fun onStats(rttMs: Float, bitrateKbps: Int, lossPercent: Float) {}
        }
    }

    fun submitFrame(data: ByteArray, ptsUs: Long, flags: Int) {
        val dec = decoder ?: return
        val mime = dec.info?.mime ?: "video/hevc"
        val key = (flags and NativeBridge.FLAG_KEYFRAME) != 0 || AnnexB.containsKeyFrame(data, mime)
        val start = SystemClock.elapsedRealtimeNanos()
        dec.submit(EncodedFrame(data, ptsUs, key, (flags and NativeBridge.FLAG_CONFIG) != 0))
        adpf?.report(SystemClock.elapsedRealtimeNanos() - start)
    }

    fun onSurfaceDestroyed() {
        NativeBridge.callbacks = null
        thermal?.stop(); thermal = null
        watchdog?.stop(); watchdog = null
        presenter?.stop(); presenter = null
        decoder?.stop(); decoder = null
        adpf?.close(); adpf = null
        surface = null
    }

    fun release() {
        onSurfaceDestroyed()
        capture?.disable(); capture = null
        view = null
    }

    /** Kích thước giải mã khởi tạo = màn hình gốc x tỉ lệ (số chẵn). TODO: lấy kích thước thật từ host. */
    private fun decodeSize(): Pair<Int, Int> {
        val dm = DisplayMetrics()
        @Suppress("DEPRECATION")
        activity.windowManager.defaultDisplay.getRealMetrics(dm)
        val k = settings.resolutionPercent / 100f
        fun even(v: Float) = (v.toInt() / 2) * 2
        return even(dm.widthPixels * k) to even(dm.heightPixels * k)
    }

    @Suppress("unused")
    private fun ctx(): Context = activity
}
