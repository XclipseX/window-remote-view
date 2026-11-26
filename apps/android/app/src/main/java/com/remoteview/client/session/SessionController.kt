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
import com.remoteview.client.audio.AudioPlayer
import com.remoteview.client.decoder.AnnexB
import com.remoteview.client.decoder.DecoderPolicy
import com.remoteview.client.decoder.EncodedFrame
import com.remoteview.client.decoder.LowLatencyDecoder
import com.remoteview.client.decoder.MimeTypes
import com.remoteview.client.decoder.StallWatchdog
import com.remoteview.client.input.DirectTouchMapper
import com.remoteview.client.input.KeyboardBridge
import com.remoteview.client.input.PointerCaptureController
import com.remoteview.client.input.StylusMapper
import com.remoteview.client.input.TouchGestureMapper
import com.remoteview.client.perf.AdpfSession
import com.remoteview.client.perf.ThermalGovernor
import com.remoteview.client.render.DisplayLock
import com.remoteview.client.render.FramePresenter
import com.remoteview.client.ui.RemoteSurfaceView
import com.remoteview.client.usb.DatagramLink
import com.remoteview.client.usb.DisplayCaps
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

// UNBUILT: chưa biên dịch.

/** Nối decoder + presenter + watchdog + ADPF + thermal + input với nhau cho một phiên. */
class SessionController(
    private val activity: Activity,
    private val settings: StreamSettings,
    /** Chế độ "Màn hình phụ (USB)": toàn màn hình, cảm ứng trực tiếp 1:1, host tạo màn hình ảo khớp. */
    val usbDisplay: Boolean = false,
) {
    @Volatile var nativeHandle: Long = 0L

    private val sink = NativeInputSink { nativeHandle }
    val keyboard = KeyboardBridge(sink).also { it.rawTyping = settings.rawTyping }
    private val stylus = StylusMapper(sink)
    private val touch = TouchGestureMapper(
        activity, sink, if (usbDisplay) com.remoteview.client.input.TouchMode.DIRECT else settings.touchMode,
    ) { s, fx, fy -> view?.applyZoom(s, fx, fy) }
    private val directTouch = if (usbDisplay) DirectTouchMapper(sink) else null
    private val baseFps = if (usbDisplay) DisplayCaps.nativeDisplay(activity).refreshHz else settings.fps
    val helloJson: String get() = DisplayCaps.helloJson(activity, settings, usbDisplay)

    private var audio: AudioPlayer? = null
    private var link: DatagramLink? = null
    /** Liên kết USB đóng (rút cáp...): MainActivity kết thúc phiên. Gọi từ thread nền. */
    var onLinkClosed: ((String?) -> Unit)? = null
    /** Lõi Rust báo phiên lỗi (sai PIN, host không trả lời...). Gọi từ thread nền. */
    var onNativeFailed: ((String?) -> Unit)? = null

    private val _reconnecting = MutableStateFlow(false)
    /** true khi host im lặng (state 4): UI hiện lớp phủ "đang kết nối lại", không thoát phiên. */
    val reconnecting: StateFlow<Boolean> = _reconnecting

    /** Cấu hình luồng do host báo (state 11). */
    private data class StreamConfig(val mime: String, val width: Int, val height: Int, val fps: Int)
    @Volatile private var streamConfig: StreamConfig? = null
    private val videoLock = Any()

    private var view: RemoteSurfaceView? = null
    private var capture: PointerCaptureController? = null
    @Volatile private var decoder: LowLatencyDecoder? = null
    private var presenter: FramePresenter? = null
    private var watchdog: StallWatchdog? = null
    private var adpf: AdpfSession? = null
    private var thermal: ThermalGovernor? = null
    private var surface: Surface? = null
    private var effectiveFps = baseFps
    private var lastFrameSubmitMs = 0L

    fun bindView(v: RemoteSurfaceView) {
        view = v
        v.bind(touch, stylus, keyboard, directTouch)
        capture = PointerCaptureController(v, sink).also { it.enable() }
    }

    /**
     * Gắn liên kết USB (AOA / ADB reverse): datagram vào -> Rust, Rust ra -> liên kết.
     * Khi liên kết mở, tạo phiên native với caps (độ phân giải gốc + tần số quét) trong Hello.
     */
    fun attachLink(l: DatagramLink) {
        link = l
        NativeBridge.datagramSender = { bytes -> l.send(bytes) }
        l.listener = object : DatagramLink.Listener {
            override fun onOpen() {
                if (NativeBridge.available) nativeHandle = NativeBridge.nativeConnectLink(helloJson)
            }
            override fun onDatagram(data: ByteArray) {
                val h = nativeHandle
                if (NativeBridge.available && h != 0L) NativeBridge.nativePushDatagram(h, data)
            }
            override fun onClosed(reason: String?) { onLinkClosed?.invoke(reason) }
        }
        l.start()
    }

    fun toggleSoftKeyboard() { view?.toggleSoftKeyboard() }

    fun onWindowFocusChanged(hasFocus: Boolean) = capture?.onWindowFocusChanged(hasFocus)

    fun onSurfaceReady(s: Surface) {
        surface = s
        DisplayLock.lockHighestRefresh(activity)
        DisplayLock.applyFrameRate(s, effectiveFps.toFloat())

        startVideo(s)

        adpf = AdpfSession(activity, intArrayOf(Process.myTid()), effectiveFps)
        thermal = ThermalGovernor(activity, object : ThermalGovernor.Listener {
            override fun onBitrateScale(scale: Float) {
                if (NativeBridge.available && nativeHandle != 0L) {
                    NativeBridge.nativeSetBitrate(nativeHandle, (settings.targetBitrateKbps() * scale).toInt())
                }
            }

            override fun onFpsCap(maxFps: Int?) {
                effectiveFps = minOf(baseFps, maxFps ?: baseFps)
                surface?.let { DisplayLock.applyFrameRate(it, effectiveFps.toFloat()) }
                adpf?.updateTarget(effectiveFps)
                if (NativeBridge.available && nativeHandle != 0L) NativeBridge.nativeSetMaxFps(nativeHandle, effectiveFps)
            }
        }).also { it.start() }

        if (settings.audioEnabled && audio == null) audio = AudioPlayer().also { it.start() }

        NativeBridge.callbacks = object : NativeBridge.Callbacks {
            override fun onVideoFrame(data: ByteArray, ptsUs: Long, flags: Int) = submitFrame(data, ptsUs, flags)
            override fun onAudioPacket(data: ByteArray, ptsUs: Long, codec: Int) { audio?.submit(data, codec) }
            override fun onConnectionState(state: Int, message: String?) = onNativeState(state, message)
            override fun onHostDiscovered(name: String, address: String) {}
            override fun onStats(rttMs: Float, bitrateKbps: Int, lossPercent: Float) {}
        }
    }

    /** Dựng decoder + presenter + watchdog theo [streamConfig] (nếu có) hoặc kích thước màn hình. */
    private fun startVideo(s: Surface) {
      synchronized(videoLock) {
        stopVideo()
        val cfg = streamConfig
        val (dw, dh) = decodeSize()
        val w = cfg?.width ?: dw
        val h = cfg?.height ?: dh
        val fps = cfg?.fps ?: effectiveFps
        val keyFrame = { if (NativeBridge.available && nativeHandle != 0L) NativeBridge.nativeRequestKeyFrame(nativeHandle) }
        val dec = LowLatencyDecoder(
            w, h, fps,
            DecoderPolicy(settings.codec, settings.allowAv1, settings.hardwareDecode, forcedMime = cfg?.mime),
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
      }
    }

    private fun stopVideo() {
      synchronized(videoLock) {
        watchdog?.stop(); watchdog = null
        presenter?.stop(); presenter = null
        decoder?.stop(); decoder = null
      }
    }

    private fun parseStreamConfig(msg: String?): StreamConfig? {
        val m = Regex("""^\s*(\w+)\s+(\d+)x(\d+)@(\d+)""").find(msg ?: return null) ?: return null
        val mime = when (m.groupValues[1].lowercase()) {
            "h264" -> MimeTypes.AVC
            "hevc" -> MimeTypes.HEVC
            "av1" -> MimeTypes.AV1
            else -> return null
        }
        val w = m.groupValues[2].toIntOrNull() ?: return null
        val h = m.groupValues[3].toIntOrNull() ?: return null
        val fps = m.groupValues[4].toIntOrNull() ?: return null
        if (w <= 0 || h <= 0 || fps <= 0) return null
        return StreamConfig(mime, w, h, fps)
    }

        fun onNativeState(state: Int, message: String?) {
        when (state) {
            NativeBridge.STATE_FAILED -> onNativeFailed?.invoke(message)
            NativeBridge.STATE_DISCONNECTED -> _reconnecting.value = true
            NativeBridge.STATE_ESTABLISHED -> {
                val was = _reconnecting.value
                _reconnecting.value = false
                if (was) {
                    // Nối lại: bỏ dữ liệu cũ, chờ keyframe tiếp theo.
                    presenter?.clear()
                    decoder?.flush()
                }
            }
            NativeBridge.STATE_STREAM_CONFIG -> {
                val cfg = parseStreamConfig(message) ?: return
                val old = streamConfig
                streamConfig = cfg
                val s = surface ?: return
                val cur = decoder
                val same = old != null && cur != null && old.mime == cfg.mime &&
                    old.width == cfg.width && old.height == cfg.height && old.fps == cfg.fps
                if (!same) startVideo(s) // mime/kích thước/fps đổi (hoặc decoder chưa đúng cấu hình): dựng lại
            }
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
        stopVideo()
        adpf?.close(); adpf = null
        surface = null
    }

    fun release() {
        onSurfaceDestroyed()
        audio?.stop(); audio = null
        link?.let { it.listener = null; it.close() }
        link = null
        NativeBridge.datagramSender = null
        capture?.disable(); capture = null
        view = null
    }

    /** Kích thước giải mã khởi tạo = màn hình gốc x tỉ lệ (số chẵn). TODO: lấy kích thước thật từ host. */
    private fun decodeSize(): Pair<Int, Int> {
        val dm = DisplayMetrics()
        @Suppress("DEPRECATION")
        activity.windowManager.defaultDisplay.getRealMetrics(dm)
        val k = if (usbDisplay) 1f else settings.resolutionPercent / 100f
        fun even(v: Float) = (v.toInt() / 2) * 2
        return even(dm.widthPixels * k) to even(dm.heightPixels * k)
    }

    @Suppress("unused")
    private fun ctx(): Context = activity
}
