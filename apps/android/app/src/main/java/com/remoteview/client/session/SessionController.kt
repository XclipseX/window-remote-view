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
import com.remoteview.client.LogShare
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
import com.remoteview.client.input.TouchMode
import com.remoteview.client.perf.AdpfSession
import com.remoteview.client.render.DisplayLock
import com.remoteview.client.render.FramePresenter
import com.remoteview.client.ui.RemoteSurfaceView
import com.remoteview.client.usb.AccessoryLink
import com.remoteview.client.usb.DatagramLink
import com.remoteview.client.usb.DeviceName
import com.remoteview.client.usb.DisplayCaps
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import java.util.concurrent.atomic.AtomicLong

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
    private val nativeHandleAtomic = AtomicLong(0L)
    val nativeHandle: Long get() = nativeHandleAtomic.get()
    @Volatile var rttMs: Float = 0f
    @Volatile var bitrateKbps: Int = 0
    @Volatile var lossPercent: Float = 0f

    private val sink = NativeInputSink { nativeHandle }
    val keyboard = KeyboardBridge(sink).also { it.rawTyping = settings.rawTyping }
    private val stylus = StylusMapper(sink)
    private val touch = TouchGestureMapper(
        activity, sink, settings.touchMode,
        onLocalMove = { dx, dy -> onLocalCursorDelta(dx, dy) },
    ) { s, fx, fy -> view?.applyZoom(s, fx, fy) }
    private val directTouch = if (usbDisplay && settings.touchMode == TouchMode.DIRECT) DirectTouchMapper(sink) else null
    private val baseFps: Int = run {
        val maxHz = DisplayCaps.nativeDisplay(activity).refreshHz
        if (maxHz > 0) minOf(settings.fps, maxHz) else settings.fps
    }
    val helloJson: String get() = DisplayCaps.helloJson(activity, settings, usbDisplay, otp, deviceName)
    private val deviceName: String = DeviceName.friendly(activity)

    private var audio: AudioPlayer? = null
    private var link: DatagramLink? = null
    @Volatile private var lanTarget: String? = null
    /** Liên kết USB đóng (rút cáp...): MainActivity kết thúc phiên. Gọi từ thread nền. */
    var onLinkClosed: ((String?) -> Unit)? = null
    /** Lõi Rust báo phiên lỗi (sai PIN, host không trả lời...). Gọi từ thread nền. */
    var onNativeFailed: ((String?) -> Unit)? = null
    /** Phiên đã bắt tay xong (STATE_ESTABLISHED). Gọi từ main thread. */
    var onEstablished: (() -> Unit)? = null

    @Volatile private var established = false
    /** true khi đã bắt tay xong (có handle + state ESTABLISHED): dùng cho nút "Gửi log lên máy tính". */
    val isConnected: Boolean get() = nativeHandle != 0L && established

    fun onNativeStats(rttMs: Float, bitrateKbps: Int, lossPercent: Float) {
        this.rttMs = rttMs
        this.bitrateKbps = bitrateKbps
        this.lossPercent = lossPercent
    }

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

    private val _hud = MutableStateFlow(HudState())
    /** Số liệu FPS cho lớp phủ; chỉ được cập nhật khi [setHudEnabled](true). */
    val hud: StateFlow<HudState> = _hud
    @Volatile private var hudEnabled = false
    private val hudTick = object : Runnable {
        override fun run() {
            if (released || !hudEnabled) return
            val pres = presenter
            val (w, h) = _streamSize.value
            _hud.value = HudState(
                stats = pres?.stats?.snapshot(System.nanoTime()) ?: com.remoteview.client.render.FrameStatsSnapshot(),
                decoderName = decoder?.info?.codecName ?: "-",
                width = w,
                height = h,
                dropped = pres?.droppedStale?.get() ?: 0L,
                rttMs = rttMs,
                decodeMs = decoder?.avgDecodeMs ?: 0f,
                bitrateKbps = bitrateKbps,
                lossPercent = lossPercent,
            )
            mainHandler.postDelayed(this, HUD_INTERVAL_MS)
        }
    }

    /** Bật/tắt bộ đo FPS. Gọi từ main thread. */
    fun setHudEnabled(on: Boolean) {
        if (hudEnabled == on) return
        hudEnabled = on
        mainHandler.removeCallbacks(hudTick)
        if (on && !released) mainHandler.post(hudTick)
    }

    private val _reconnecting = MutableStateFlow(false)
    /** true khi host im lặng (state 4) cho đến khi khung hình mới sau kết nối lại được giải mã: UI hiện lớp phủ "đang kết nối lại". */
    val reconnecting: StateFlow<Boolean> = _reconnecting
    /** Chặn các khung cũ còn sót trong pipeline MediaCodec tắt nhãn "đang kết nối lại" trước khi khung mới sau reconnect tới. */
    @Volatile private var reconnectFrameBarrier = false

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
    @Volatile private var adpf: AdpfSession? = null
    private var surface: Surface? = null
    @Volatile private var effectiveFps = baseFps
    @Volatile private var currentQuality = settings.qualityPercent
    @Volatile private var lastLogUploadReqMs = 0L
    @Volatile private var lastDropNoHandleMs = 0L

    val liveQuality: Int get() = currentQuality
    val liveFps: Int get() = effectiveFps

    // --- Số liệu gỡ lỗi ---
    @Volatile private var dbgState = -1
    @Volatile private var dbgMessage = ""
    @Volatile private var dbgLastError = ""
    @Volatile private var dbgEstablishedAt = 0L
    @Volatile private var dbgHandshake = "chưa mở liên kết"
    private val dbgFramesSubmitted = AtomicLong()

    fun debugSnapshot(): DebugSnapshot {
        val l = link
        val aoa = l as? AccessoryLink
        val lan = lanTarget
        return DebugSnapshot(
            linkType = l?.name ?: lan?.let { "LAN ($it)" } ?: "-",
            linkOpen = l?.isOpen == true || (lan != null && nativeHandle != 0L),
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
            rttMs = rttMs,
            decodeMs = decoder?.avgDecodeMs ?: 0f,
            bitrateKbps = bitrateKbps,
            lossPercent = lossPercent,
        )
    }

    fun recordError(msg: String) { dbgLastError = msg; RvLog.e("phiên: $msg") }

    fun onAudio(data: ByteArray, codec: Int) { audio?.submit(data, codec) }

    fun bindView(v: RemoteSurfaceView) {
        view = v
        v.bind(touch, stylus, keyboard, directTouch)
        capture = PointerCaptureController(v, sink) { dx, dy -> onLocalCursorDelta(dx, dy) }.also { it.enable() }
    }

    /** Đóng phiên native một lần duy nhất trên thread nền (không chặn UI thread). */
    private fun disconnectNativeAsync() {
        val h = nativeHandleAtomic.getAndSet(0L)
        if (h != 0L && NativeBridge.available) {
            Thread({
                try {
                    NativeBridge.nativeDisconnect(h)
                } catch (t: Throwable) {
                    RvLog.w("nativeDisconnect($h) lỗi", t)
                }
            }, "rv-native-disconnect").also { it.isDaemon = true; it.start() }
        }
    }

    /**
     * Gắn liên kết USB (AOA / ADB reverse): datagram vào -> Rust, Rust ra -> liên kết.
     * Khi liên kết mở, tạo phiên native với caps (độ phân giải gốc + tần số quét) trong Hello.
     */
    fun attachLink(l: DatagramLink) {
        if (released) {
            l.close()
            return
        }
        link = l
        RvLog.i("attachLink: ${l.name}, native=${NativeBridge.available}, otp=${if (otp != null) "tự động" else "PIN lưu"}")
        // Gói Rust gửi ra phải được ghi vào liên kết NGAY từ khi phiên native tạo xong: đặt sender trước và gắn chủ sở hữu.
        NativeBridge.setDatagramSender(this) { bytes ->
            if (released || !NativeBridge.isDatagramSenderOwner(this)) return@setDatagramSender
            val ok = l.send(bytes)
            if (!ok) RvLog.w("datagramSender: liên kết từ chối ${bytes.size}B")
        }
        l.listener = object : DatagramLink.Listener {
            override fun onOpen() {
                if (released) {
                    RvLog.i("liên kết ${l.name} mở sau khi phiên đã release -> bỏ qua")
                    return
                }
                RvLog.i("liên kết ${l.name} đã mở -> tạo phiên native")
                if (!NativeBridge.available) {
                    recordError("librv_core.so chưa nạp, không thể bắt tay")
                    dbgHandshake = "không có lõi native"
                    return
                }
                val h = try {
                    NativeBridge.nativeConnectLink(helloJson)
                } catch (t: Throwable) {
                    if (released) return
                    recordError("nativeConnectLink ném lỗi: ${t.javaClass.simpleName}: ${t.message}")
                    dbgHandshake = "lỗi native"
                    onNativeFailed?.invoke(t.message)
                    return
                }
                if (released) {
                    RvLog.w("nativeConnectLink trả handle=$h sau khi phiên đã release -> hủy trên nền")
                    if (h != 0L) {
                        Thread({ runCatching { NativeBridge.nativeDisconnect(h) } }, "rv-native-disconnect")
                            .also { it.isDaemon = true; it.start() }
                    }
                    return
                }
                nativeHandleAtomic.set(h)
                if (released) {
                    disconnectNativeAsync()
                    return
                }
                RvLog.i("nativeConnectLink -> handle=$h")
                if (h == 0L) {
                    recordError("nativeConnectLink trả 0 (hello/OTP không hợp lệ?)")
                    dbgHandshake = "native từ chối tạo phiên"
                    onNativeFailed?.invoke("lõi từ chối tạo phiên (OTP?)")
                } else {
                    dbgHandshake = "đã tạo phiên, chờ host trả lời Hello"
                }
            }
            override fun onDatagram(data: ByteArray) {
                if (released) return
                val h = nativeHandle
                if (NativeBridge.available && h != 0L) {
                    try { NativeBridge.nativePushDatagram(h, data) } catch (t: Throwable) {
                        recordError("nativePushDatagram: ${t.message}")
                    }
                } else {
                    val now = SystemClock.elapsedRealtime()
                    if (now - lastDropNoHandleMs >= 2000L) {
                        lastDropNoHandleMs = now
                        RvLog.w("datagram ${data.size}B tới khi chưa có phiên native (handle=$h) -> bỏ")
                    }
                }
            }
            override fun onClosed(reason: String?) {
                if (released) return
                RvLog.w("liên kết ${l.name} đóng: ${reason ?: "-"}")
                if (reason != null) dbgLastError = reason
                dbgHandshake = "liên kết đã đóng"
                onLinkClosed?.invoke(reason)
            }
        }
        l.start()
    }

    /**
     * Kết nối qua mạng LAN (UDP trực tiếp tới [target]) bằng lõi Rust.
     * Dùng [otp] đã truyền vào constructor (gửi trong [helloJson]).
     */
    fun connectLan(target: String) {
        if (released) return
        lanTarget = target
        RvLog.i("connectLan: target=$target, native=${NativeBridge.available}, otp=${if (otp != null) "đã nhập" else "PIN lưu"}")
        if (!NativeBridge.available) {
            val msg = "librv_core.so chưa nạp, không thể kết nối LAN"
            recordError(msg)
            dbgHandshake = "không có lõi native"
            onNativeFailed?.invoke(msg)
            return
        }
        dbgHandshake = "đang kết nối LAN ($target)"
        val h = try {
            NativeBridge.nativeConnect(target, helloJson)
        } catch (t: Throwable) {
            if (released) return
            recordError("nativeConnect ném lỗi: ${t.javaClass.simpleName}: ${t.message}")
            dbgHandshake = "lỗi native"
            onNativeFailed?.invoke(t.message ?: "lỗi nativeConnect")
            return
        }
        if (released) {
            if (h != 0L) {
                Thread({ runCatching { NativeBridge.nativeDisconnect(h) } }, "rv-native-disconnect")
                    .also { it.isDaemon = true; it.start() }
            }
            return
        }
        nativeHandleAtomic.set(h)
        if (released) {
            disconnectNativeAsync()
            return
        }
        RvLog.i("nativeConnect($target) -> handle=$h")
        if (h == 0L) {
            recordError("nativeConnect trả 0 (địa chỉ hoặc PIN không hợp lệ?)")
            dbgHandshake = "native từ chối tạo phiên LAN"
            onNativeFailed?.invoke("không thể kết nối tới $target")
        } else {
            dbgHandshake = "đã gửi Hello tới $target"
        }
    }

    /** Yêu cầu host đổi chất lượng (1..100) và FPS trực tiếp trong phiên đang chạy (fps <= 0 giữ nguyên). */
    fun applyStreamQuality(quality: Int, fps: Int) {
        val h = nativeHandle
        val q = quality.coerceIn(1, 100)
        val f = if (fps > 0) fps.coerceIn(15, 240) else 0
        currentQuality = q
        if (f > 0) {
            val maxHz = DisplayCaps.nativeDisplay(activity).refreshHz
            effectiveFps = if (maxHz > 0) minOf(f, maxHz) else f
            surface?.let { DisplayLock.applyFrameRate(it, effectiveFps.toFloat()) }
            adpf?.updateTarget(effectiveFps)
        }
        if (NativeBridge.available && h != 0L) {
            NativeBridge.nativeSetStreamQuality(h, q, 0, 0, f)
        }
    }

    fun toggleSoftKeyboard() { view?.toggleSoftKeyboard() }

    fun onWindowFocusChanged(hasFocus: Boolean) = capture?.onWindowFocusChanged(hasFocus)

    private fun refreshAdpf(codecTid: Int = 0) {
        val tid = if (codecTid > 0) codecTid else Process.myTid()
        val prev = adpf
        adpf = null
        prev?.close()
        adpf = AdpfSession(activity, intArrayOf(tid), effectiveFps)
    }

    fun onSurfaceReady(s: Surface) {
        surface = s
        DisplayLock.lockHighestRefresh(activity)
        DisplayLock.applyFrameRate(s, effectiveFps.toFloat())

        // Chỉ dựng decoder khi đã biết cấu hình luồng; chưa có thì chờ STATE_STREAM_CONFIG (hoặc dự phòng).
        if (streamConfig != null) startVideo(s) else armConfigFallback()
        if (adpf == null) refreshAdpf()

        if (settings.audioEnabled && audio == null) audio = AudioPlayer().also { it.start() }
    }

    private fun armConfigFallback() {
        mainHandler.removeCallbacks(configFallback)
        if (established && surface != null && streamConfig == null) {
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
        val keyFrame = {
            val handle = nativeHandle
            if (NativeBridge.available && handle != 0L) NativeBridge.nativeRequestKeyFrame(handle)
        }
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
        // Không hạ tần số quét nhân tạo khi tĩnh — để SurfaceFlinger và LTPO/PSR phần cứng tự tối ưu.
        val pres = FramePresenter(
            dec,
            if (settings.directPresent) FramePresenter.Mode.DIRECT else FramePresenter.Mode.CHOREOGRAPHER,
            onIdleChanged = null,
        )
        dec.onOutput = { index, ptsUs ->
            if (established && _reconnecting.value && !reconnectFrameBarrier) {
                _reconnecting.value = false
            }
            pres.onOutput(index, ptsUs)
        }
        dec.onDecodeTiming = { decodeUs ->
            val handle = nativeHandle
            if (NativeBridge.available && handle != 0L) {
                NativeBridge.nativeReportDecodeUs(handle, decodeUs)
            }
            adpf?.report(decodeUs * 1000L)
        }
        decoder = dec
        presenter = pres
        val started = dec.start(s)
        videoStarted = started
        RvLog.i(
            "decoder start=$started name=${dec.info?.codecName} tier=${dec.info?.tierLabel ?: "-"} " +
                "hwPref=${settings.hardwareDecode} direct=${settings.directPresent} ${w}x$h@$fps tid=${dec.codecThreadTid}",
        )
        if (!started) dbgLastError = "decoder không khởi động được"
        if (started) {
            // Đăng ký luồng giải mã MediaCodec thực tế vào ADPF để governor tăng xung đúng thread.
            refreshAdpf(dec.codecThreadTid)
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
            if (!released) handleNativeState(state, message)
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
            NativeBridge.STATE_CLOSED -> { established = false; dbgHandshake = "đã đóng" }
            NativeBridge.STATE_FAILED -> { established = false; dbgHandshake = "thất bại"; dbgLastError = message ?: "native FAILED" }
            NativeBridge.STATE_DISCONNECTED -> { established = false; dbgHandshake = "host im lặng" }
            NativeBridge.STATE_ESTABLISHED -> {
                if (_reconnecting.value) {
                    // Nối lại: giữ lớp phủ "đang kết nối lại" qua ESTABLISHED tới khi khung hình mới giải mã xong.
                    reconnectFrameBarrier = true
                    presenter?.clear()
                    decoder?.flush()
                }
                established = true
                dbgHandshake = "đã bắt tay"
                if (dbgEstablishedAt == 0L) dbgEstablishedAt = SystemClock.elapsedRealtime()
            }
        }
        when (state) {
            NativeBridge.STATE_FAILED -> onNativeFailed?.invoke(message)
            NativeBridge.STATE_DISCONNECTED -> {
                link?.clearWriteQueue()
                reconnectFrameBarrier = true
                _reconnecting.value = true
            }
            NativeBridge.STATE_ESTABLISHED -> {
                val h = nativeHandle
                if (NativeBridge.available && h != 0L) NativeBridge.nativeRequestKeyFrame(h)
                armConfigFallback()
                onEstablished?.invoke()
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
            NativeBridge.STATE_LOG_REQUEST -> {
                val h = nativeHandle
                val now = SystemClock.elapsedRealtime()
                if (h != 0L && NativeBridge.available && now - lastLogUploadReqMs >= LOG_UPLOAD_MIN_INTERVAL_MS) {
                    lastLogUploadReqMs = now
                    Thread({
                        val ok = LogShare.upload(h)
                        if (ok) {
                            RvLog.i("host yêu cầu log -> đã gửi lên máy tính")
                        } else {
                            RvLog.w("host yêu cầu log -> gửi thất bại")
                        }
                    }, "rv-log-host-req").also { it.isDaemon = true; it.start() }
                }
            }
        }
    }

    fun submitFrame(data: ByteArray, ptsUs: Long, flags: Int) {
        dbgFramesSubmitted.incrementAndGet()
        val dec = decoder ?: return
        if (established && reconnectFrameBarrier) {
            reconnectFrameBarrier = false
        }
        val mime = dec.info?.mime ?: "video/hevc"
        val key = (flags and NativeBridge.FLAG_KEYFRAME) != 0 || AnnexB.containsKeyFrame(data, mime)
        dec.submit(EncodedFrame(data, ptsUs, key, (flags and NativeBridge.FLAG_CONFIG) != 0))
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
        stopVideo()
        adpf?.close(); adpf = null
        surface = null
    }

    fun release() {
        released = true
        established = false
        mainHandler.removeCallbacksAndMessages(null)
        onSurfaceDestroyed()
        audio?.stop(); audio = null
        link?.let { it.listener = null; it.close() }
        link = null
        NativeBridge.clearDatagramSender(this)
        disconnectNativeAsync()
        capture?.disable(); capture = null
        view = null
    }

    /** Kích thước giải mã khởi tạo = màn hình gốc x tỉ lệ (số chẵn). */
    private fun decodeSize(): Pair<Int, Int> {
        val dm = DisplayMetrics()
        @Suppress("DEPRECATION")
        activity.windowManager.defaultDisplay.getRealMetrics(dm)
        val k = settings.resolutionPercent.coerceIn(25, 100) / 100f
        fun even(v: Float) = ((v.toInt() / 2) * 2).coerceAtLeast(2)
        return even(dm.widthPixels * k) to even(dm.heightPixels * k)
    }

    @Suppress("unused")
    private fun ctx(): Context = activity

    private companion object {
        const val CONFIG_WAIT_MS = 1500L
        const val HUD_INTERVAL_MS = 500L
        const val LOG_UPLOAD_MIN_INTERVAL_MS = 3000L
    }
}
