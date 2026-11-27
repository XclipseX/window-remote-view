package com.remoteview.client.session

import android.app.Activity
import android.content.Context
import android.graphics.Bitmap
import android.os.Handler
import android.os.Looper
import android.os.Process
import android.os.SystemClock
import android.util.DisplayMetrics
import android.view.Surface
import com.remoteview.client.NativeBridge
import com.remoteview.client.NativeInputSink
import com.remoteview.client.RvLog
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
import com.remoteview.client.usb.AccessoryLink
import com.remoteview.client.usb.DatagramLink
import com.remoteview.client.usb.DeviceName
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
    /** OTP tự động (accessory.serial); null = dùng PIN đã lưu trong cài đặt. */
    private val otp: String? = null,
) {
    @Volatile var nativeHandle: Long = 0L

    private val sink = NativeInputSink { nativeHandle }
    val keyboard = KeyboardBridge(sink).also { it.rawTyping = settings.rawTyping }
    private val stylus = StylusMapper(sink)
    private val touch = TouchGestureMapper(
        activity, sink, if (usbDisplay) com.remoteview.client.input.TouchMode.DIRECT else settings.touchMode,
        onLocalMove = { dx, dy -> onLocalCursorDelta(dx, dy) },
    ) { s, fx, fy -> view?.applyZoom(s, fx, fy) }
    private val directTouch = if (usbDisplay) DirectTouchMapper(sink) else null
    private val baseFps = if (usbDisplay) DisplayCaps.nativeDisplay(activity).refreshHz else settings.fps
    val helloJson: String get() = DisplayCaps.helloJson(activity, settings, usbDisplay, otp, deviceName)
    private val deviceName: String = DeviceName.friendly(activity)

    private var audio: AudioPlayer? = null
    private var link: DatagramLink? = null
    /** Liên kết USB đóng (rút cáp...): MainActivity kết thúc phiên. Gọi từ thread nền. */
    var onLinkClosed: ((String?) -> Unit)? = null
    /** Lõi Rust báo phiên lỗi (sai PIN, host không trả lời...). Gọi từ thread nền. */
    var onNativeFailed: ((String?) -> Unit)? = null

    /** true khi đã bắt tay xong (có handle + state ESTABLISHED): dùng cho nút "Gửi log lên máy tính". */
    val isConnected: Boolean get() = nativeHandle != 0L && dbgState == NativeBridge.STATE_ESTABLISHED

    /** Kích thước khung hình luồng (WxH) để UI đặt SurfaceView đúng tỉ lệ; ban đầu = kích thước màn hình. */
    private val _streamSize = MutableStateFlow(decodeSize())
    val streamSize: StateFlow<Pair<Int, Int>> = _streamSize

    private val _cursor = MutableStateFlow(CursorState())
    /** Con trỏ chuột của host (hình + vị trí 0..65535 trên khung hình) để vẽ lớp phủ. */
    val cursor: StateFlow<CursorState> = _cursor

    fun onCursorShape(w: Int, h: Int, hotX: Int, hotY: Int, rgba: ByteArray) {
        if (w <= 0 || h <= 0 || w > 512 || h > 512 || rgba.size < w * h * 4) return
        try {
            val px = IntArray(w * h)
            for (i in px.indices) {
                val o = i * 4
                // Host gửi RGBA alpha thẳng; Bitmap ARGB_8888 của Android là premultiplied -> nhân sẵn để không viền trắng.
                val a = rgba[o + 3].toInt() and 0xFF
                val r = ((rgba[o].toInt() and 0xFF) * a + 127) / 255
                val g = ((rgba[o + 1].toInt() and 0xFF) * a + 127) / 255
                val b = ((rgba[o + 2].toInt() and 0xFF) * a + 127) / 255
                px[i] = (a shl 24) or (r shl 16) or (g shl 8) or b
            }
            val bmp = Bitmap.createBitmap(px, w, h, Bitmap.Config.ARGB_8888)
            _cursor.value = _cursor.value.copy(bitmap = bmp, hotX = hotX, hotY = hotY)
        } catch (t: Throwable) {
            RvLog.w("onCursorShape lỗi", t)
        }
    }

    fun onCursorPos(x: Int, y: Int, visible: Boolean) {
        _cursor.value = _cursor.value.copy(x = x.coerceIn(0, 65535), y = y.coerceIn(0, 65535), visible = visible)
    }

    /** Chế độ trackpad: dịch con trỏ cục bộ ngay (dx,dy là pixel của khung hình host) để không có độ trễ cảm nhận. */
    private fun onLocalCursorDelta(dx: Int, dy: Int) {
        val (w, h) = _streamSize.value
        if (w <= 0 || h <= 0) return
        val c = _cursor.value
        _cursor.value = c.copy(
            x = (c.x + dx * 65535f / w).toInt().coerceIn(0, 65535),
            y = (c.y + dy * 65535f / h).toInt().coerceIn(0, 65535),
            visible = true,
        )
    }

    private val _reconnecting = MutableStateFlow(false)
    /** true khi host im lặng (state 4): UI hiện lớp phủ "đang kết nối lại", không thoát phiên. */
    val reconnecting: StateFlow<Boolean> = _reconnecting

    /** Cấu hình luồng do host báo (state 11). */
    private data class StreamConfig(val mime: String, val width: Int, val height: Int, val fps: Int)
    @Volatile private var streamConfig: StreamConfig? = null
    private val videoLock = Any()
    private val mainHandler = Handler(Looper.getMainLooper())
    @Volatile private var released = false
    /** true khi decoder hiện tại đã start() thành công (để StreamConfig cùng cấu hình vẫn dựng lại nếu lần trước hỏng). */
    @Volatile private var videoStarted = false
    /** Dự phòng cho host cũ không gửi StreamConfig: sau ESTABLISHED chờ tối đa [CONFIG_WAIT_MS] rồi dựng decoder mặc định. */
    private val configFallback = Runnable {
        val sf = surface
        if (!released && sf != null && streamConfig == null && !videoStarted) {
            RvLog.w("không có StreamConfig sau ${CONFIG_WAIT_MS}ms -> dựng decoder mặc định (host cũ)")
            startVideo(sf)
        }
    }

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

    // --- Số liệu gỡ lỗi ---
    @Volatile private var dbgState = -1
    @Volatile private var dbgMessage = ""
    @Volatile private var dbgLastError = ""
    @Volatile private var dbgEstablishedAt = 0L
    @Volatile private var dbgHandshake = "chưa mở liên kết"
    private val dbgFramesSubmitted = java.util.concurrent.atomic.AtomicLong()

    fun debugSnapshot(): DebugSnapshot {
        val l = link
        val aoa = l as? AccessoryLink
        return DebugSnapshot(
            linkType = l?.name ?: "-",
            linkOpen = l?.isOpen == true,
            aoaOpened = if (aoa == null) "-" else if (aoa.fdOpened) "có" else if (aoa.permissionGranted) "đang mở" else "chờ quyền",
            bytesIn = l?.bytesIn ?: 0,
            bytesOut = l?.bytesOut ?: 0,
            datagramsIn = l?.datagramsIn ?: 0,
            datagramsOut = l?.datagramsOut ?: 0,
            handshake = dbgHandshake,
            nativeState = dbgState,
            nativeMessage = dbgMessage,
            nativeHandle = nativeHandle,
            decoderName = decoder?.info?.codecName ?: "-",
            framesSubmitted = dbgFramesSubmitted.get(),
            framesDecoded = decoder?.framesOut?.get() ?: 0,
            lastError = dbgLastError,
            establishedAtMs = dbgEstablishedAt,
        )
    }

    fun recordError(msg: String) { dbgLastError = msg; RvLog.e("phiên: $msg") }

    fun onAudio(data: ByteArray, codec: Int) { audio?.submit(data, codec) }

    fun bindView(v: RemoteSurfaceView) {
        view = v
        v.bind(touch, stylus, keyboard, directTouch)
        capture = PointerCaptureController(v, sink) { dx, dy -> onLocalCursorDelta(dx, dy) }.also { it.enable() }
    }

    /**
     * Gắn liên kết USB (AOA / ADB reverse): datagram vào -> Rust, Rust ra -> liên kết.
     * Khi liên kết mở, tạo phiên native với caps (độ phân giải gốc + tần số quét) trong Hello.
     */
    fun attachLink(l: DatagramLink) {
        link = l
        RvLog.i("attachLink: ${l.name}, native=${NativeBridge.available}, otp=${if (otp != null) "tự động" else "PIN lưu"}")
        // Gói Rust gửi ra phải được ghi vào liên kết NGAY từ khi phiên native tạo xong: đặt sender trước.
        NativeBridge.datagramSender = { bytes ->
            val ok = l.send(bytes)
            if (!ok) RvLog.w("datagramSender: liên kết từ chối ${bytes.size}B")
        }
        l.listener = object : DatagramLink.Listener {
            override fun onOpen() {
                RvLog.i("liên kết ${l.name} đã mở -> tạo phiên native")
                if (!NativeBridge.available) {
                    recordError("librv_core.so chưa nạp, không thể bắt tay")
                    dbgHandshake = "không có lõi native"
                    return
                }
                try {
                    nativeHandle = NativeBridge.nativeConnectLink(helloJson)
                } catch (t: Throwable) {
                    recordError("nativeConnectLink ném lỗi: ${t.javaClass.simpleName}: ${t.message}")
                    dbgHandshake = "lỗi native"
                    onNativeFailed?.invoke(t.message)
                    return
                }
                RvLog.i("nativeConnectLink -> handle=$nativeHandle")
                if (nativeHandle == 0L) {
                    recordError("nativeConnectLink trả 0 (hello/OTP không hợp lệ?)")
                    dbgHandshake = "native từ chối tạo phiên"
                    onNativeFailed?.invoke("lõi từ chối tạo phiên (OTP?)")
                } else {
                    dbgHandshake = "đã tạo phiên, chờ host trả lời Hello"
                }
            }
            override fun onDatagram(data: ByteArray) {
                val h = nativeHandle
                if (NativeBridge.available && h != 0L) {
                    try { NativeBridge.nativePushDatagram(h, data) } catch (t: Throwable) {
                        recordError("nativePushDatagram: ${t.message}")
                    }
                } else {
                    RvLog.w("datagram ${data.size}B tới khi chưa có phiên native (handle=$h) -> bỏ")
                }
            }
            override fun onClosed(reason: String?) {
                RvLog.w("liên kết ${l.name} đóng: ${reason ?: "-"}")
                if (reason != null) dbgLastError = reason
                dbgHandshake = "liên kết đã đóng"
                onLinkClosed?.invoke(reason)
            }
        }
        l.start()
    }

    fun toggleSoftKeyboard() { view?.toggleSoftKeyboard() }

    fun onWindowFocusChanged(hasFocus: Boolean) = capture?.onWindowFocusChanged(hasFocus)

    fun onSurfaceReady(s: Surface) {
        surface = s
        DisplayLock.lockHighestRefresh(activity)
        DisplayLock.applyFrameRate(s, effectiveFps.toFloat())

        // Chỉ dựng decoder khi đã biết cấu hình luồng; chưa có thì chờ STATE_STREAM_CONFIG (hoặc dự phòng).
        if (streamConfig != null) startVideo(s) else armConfigFallback()

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
    }

    private fun armConfigFallback() {
        mainHandler.removeCallbacks(configFallback)
        if (dbgState == NativeBridge.STATE_ESTABLISHED && surface != null && streamConfig == null) {
            mainHandler.postDelayed(configFallback, CONFIG_WAIT_MS)
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
        val started = dec.start(s)
        videoStarted = started
        RvLog.i("decoder start=$started name=${dec.info?.codecName} ${w}x$h@$fps")
        if (!started) dbgLastError = "decoder không khởi động được"
        if (started) {
            // Frame tới trước khi decoder sẵn sàng đã bị bỏ: xin keyframe để không bị đen.
            keyFrame()
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
        videoStarted = false
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

    /**
     * Callback từ thread "rv-session" của Rust (không có Looper): mọi thứ đụng decoder/presenter/UI
     * phải chạy trên main thread (Choreographer cần Looper), nên chuyển sang main và giữ nguyên thứ tự.
     */
    fun onNativeState(state: Int, message: String?) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            handleNativeState(state, message)
        } else {
            mainHandler.post { if (!released) handleNativeState(state, message) }
        }
    }

    private fun handleNativeState(state: Int, message: String?) {
        RvLog.i("native state=$state msg=${message ?: ""}")
        dbgState = state
        dbgMessage = message ?: ""
        when (state) {
            NativeBridge.STATE_CONNECTING -> dbgHandshake = "đang bắt tay"
            NativeBridge.STATE_CLOSED -> dbgHandshake = "đã đóng"
            NativeBridge.STATE_FAILED -> { dbgHandshake = "thất bại"; dbgLastError = message ?: "native FAILED" }
            NativeBridge.STATE_DISCONNECTED -> dbgHandshake = "host im lặng"
            NativeBridge.STATE_ESTABLISHED -> { dbgHandshake = "đã bắt tay"; if (dbgEstablishedAt == 0L) dbgEstablishedAt = SystemClock.elapsedRealtime() }
        }
        when (state) {
            NativeBridge.STATE_FAILED -> onNativeFailed?.invoke(message)
            NativeBridge.STATE_DISCONNECTED -> _reconnecting.value = true
            NativeBridge.STATE_ESTABLISHED -> {
                if (NativeBridge.available && nativeHandle != 0L) NativeBridge.nativeRequestKeyFrame(nativeHandle)
                armConfigFallback()
                val was = _reconnecting.value
                _reconnecting.value = false
                if (was) {
                    // Nối lại: bỏ dữ liệu cũ, chờ keyframe tiếp theo.
                    presenter?.clear()
                    decoder?.flush()
                }
            }
            NativeBridge.STATE_DISPLAY_CONFIG -> {
                // Chỉ dùng làm tỉ lệ tạm khi chưa có StreamConfig.
                if (streamConfig == null) {
                    val m = Regex("""(\d+)\s*x\s*(\d+)""").find(message ?: "")
                    val w = m?.groupValues?.get(1)?.toIntOrNull() ?: 0
                    val h = m?.groupValues?.get(2)?.toIntOrNull() ?: 0
                    if (w > 0 && h > 0) _streamSize.value = w to h
                }
            }
            NativeBridge.STATE_STREAM_CONFIG -> {
                val cfg = parseStreamConfig(message) ?: return
                val old = streamConfig
                streamConfig = cfg
                _streamSize.value = cfg.width to cfg.height
                val s = surface ?: return
                mainHandler.removeCallbacks(configFallback)
                val same = old != null && videoStarted && old.mime == cfg.mime &&
                    old.width == cfg.width && old.height == cfg.height && old.fps == cfg.fps
                if (!same) startVideo(s) // mime/kích thước/fps đổi (hoặc decoder chưa đúng cấu hình): dựng lại
            }
        }
    }

    fun submitFrame(data: ByteArray, ptsUs: Long, flags: Int) {
        dbgFramesSubmitted.incrementAndGet()
        val dec = decoder ?: return
        val mime = dec.info?.mime ?: "video/hevc"
        val key = (flags and NativeBridge.FLAG_KEYFRAME) != 0 || AnnexB.containsKeyFrame(data, mime)
        val start = SystemClock.elapsedRealtimeNanos()
        dec.submit(EncodedFrame(data, ptsUs, key, (flags and NativeBridge.FLAG_CONFIG) != 0))
        adpf?.report(SystemClock.elapsedRealtimeNanos() - start)
    }

    /** Activity ra nền (onStop): dừng âm thanh (surface bị hủy -> video cũng dừng). */
    fun onAppBackground() {
        audio?.stop(); audio = null
    }

    /** Activity trở lại (onStart): mở lại âm thanh nếu đã có surface; nếu chưa thì onSurfaceReady sẽ mở. */
    fun onAppForeground() {
        if (!released && surface != null && settings.audioEnabled && audio == null) {
            audio = AudioPlayer().also { it.start() }
        }
    }

    fun onSurfaceDestroyed() {
        mainHandler.removeCallbacks(configFallback)
        thermal?.stop(); thermal = null
        stopVideo()
        adpf?.close(); adpf = null
        surface = null
    }

    fun release() {
        released = true
        mainHandler.removeCallbacksAndMessages(null)
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

    private companion object {
        const val CONFIG_WAIT_MS = 1500L
    }
}
