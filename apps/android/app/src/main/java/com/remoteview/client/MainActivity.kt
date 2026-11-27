package com.remoteview.client

import android.Manifest
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import androidx.core.content.ContextCompat
import com.remoteview.client.usb.AccessoryLink
import com.remoteview.client.ui.LogScreen
import com.remoteview.client.ui.UsbHostInfo
import android.hardware.usb.UsbManager
import android.os.Build
import android.os.Bundle
import android.os.PowerManager
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.remoteview.client.bench.BenchScreen
import com.remoteview.client.net.LanLocks
import com.remoteview.client.net.NetworkWatcher
import com.remoteview.client.net.UsbTetherDetector
import com.remoteview.client.session.SessionController
import com.remoteview.client.usb.LinkSelector
import com.remoteview.client.usb.Route
import com.remoteview.client.ui.DiagnosticsScreen
import com.remoteview.client.ui.HomeScreen
import com.remoteview.client.ui.LanHost
import com.remoteview.client.ui.PinPrompt
import com.remoteview.client.ui.RemoteViewTheme
import com.remoteview.client.ui.SessionScreen
import com.remoteview.client.ui.SettingsScreen
import com.remoteview.client.ui.AppShell
import com.remoteview.client.ui.UpdateDialog
import com.remoteview.client.ui.rememberWidthClass
import com.remoteview.client.update.UpdateController
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel

// UNBUILT: chưa biên dịch.

internal enum class Screen { HOME, SETTINGS, SESSION, DIAG, BENCH, LOG }

class MainActivity : ComponentActivity() {

    private lateinit var locks: LanLocks
    private lateinit var watcher: NetworkWatcher
    private lateinit var repo: SettingsRepository
    private lateinit var recentRepo: RecentHostsRepository
    private lateinit var update: UpdateController
    private val uiScope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private val recents = mutableStateListOf<LanHost>()
    private var session: SessionController? = null
    private val lanHosts = mutableStateListOf<LanHost>()
    private var screen by mutableStateOf(Screen.HOME)
    private var settings by mutableStateOf(StreamSettings())
    private var usbStarting by mutableStateOf(false)
    private var usbHost by mutableStateOf<UsbHostInfo?>(null)
    private var usbError by mutableStateOf<String?>(null)
    /** Khóa accessory đã tự kết nối: mỗi lần cắm chỉ tự vào một lần (bấm Back ra Home không bị kéo lại). */
    private var autoTriedKey: String? = null

    private val detachReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            RvLog.w("USB accessory bị rút (broadcast)")
            runOnUiThread { usbHost = null; autoTriedKey = null }
        }
    }
    private var detachRegistered = false
    private var pinPrompt by mutableStateOf<PinPrompt?>(null)
    /** Giữ CPU thức khi phiên đang chạy dù Activity ở nền (cần quyền WAKE_LOCK). */
    private var sessionWake: PowerManager.WakeLock? = null

    private val permissionLauncher =
        registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { /* kết quả: bỏ qua, mạng vẫn chạy */ }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        locks = LanLocks(this)
        watcher = NetworkWatcher(this)
        repo = SettingsRepository(this)
        settings = repo.load()
        recentRepo = RecentHostsRepository(this)
        recents.addAll(recentRepo.load().map { LanHost(it.first, it.second) })
        update = UpdateController(this, uiScope)
        if (settings.autoCheckUpdate) update.autoCheck()
        requestNetworkPermissions()

        NativeBridge.callbacks = object : NativeBridge.Callbacks {
            override fun onVideoFrame(data: ByteArray, ptsUs: Long, flags: Int) {
                session?.submitFrame(data, ptsUs, flags)
            }
            override fun onConnectionState(state: Int, message: String?) {
                // SessionController tự chuyển sang main thread (callback này chạy trên thread Rust không có Looper).
                session?.onNativeState(state, message)
            }
            override fun onHostDiscovered(name: String, address: String) {
                runOnUiThread {
                    val idx = lanHosts.indexOfFirst { it.address == address }
                    if (idx < 0) lanHosts.add(LanHost(name, address))
                    else if (lanHosts[idx].name != name) lanHosts[idx] = LanHost(name, address)
                }
            }
            override fun onStats(rttMs: Float, bitrateKbps: Int, lossPercent: Float) {
                session?.onNativeStats(rttMs, bitrateKbps, lossPercent)
            }
            override fun onAudioPacket(data: ByteArray, ptsUs: Long, codec: Int) {
                session?.onAudio(data, codec)
            }
            override fun onCursorShape(w: Int, h: Int, hotX: Int, hotY: Int, rgba: ByteArray) {
                session?.onCursorShape(w, h, hotX, hotY, rgba)
            }
            override fun onCursorPos(x: Int, y: Int, visible: Boolean) {
                session?.onCursorPos(x, y, visible)
            }
        }
        RvLog.i("MainActivity.onCreate, API ${Build.VERSION.SDK_INT}, native=${NativeBridge.available}, intent=${intent?.action}")

        setContent {
            RemoteViewTheme { App() }
        }
        // "Cắm là hiện": mở app bằng intent USB_ACCESSORY_ATTACHED -> tự vào phiên màn hình phụ.
        // Nếu mở từ launcher khi accessory đã cắm sẵn thì onResume -> refreshAccessory cũng bắt được.
        handleUsbIntent(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        RvLog.i("onNewIntent action=${intent.action}")
        handleUsbIntent(intent)
    }

    private fun handleUsbIntent(intent: Intent?) {
        if (intent?.action == UsbManager.ACTION_USB_ACCESSORY_ATTACHED) {
            RvLog.i("nhận USB_ACCESSORY_ATTACHED")
            refreshAccessory()
        }
    }

    /** Đọc accessoryList: cập nhật thẻ "Máy tính ... đã cắm USB" và tự kết nối (một lần mỗi lần cắm). */
    private fun refreshAccessory() {
        val acc = AccessoryLink.findAccessory(this)
        if (acc == null) {
            if (usbHost != null) RvLog.i("accessory không còn trong danh sách")
            usbHost = null
            autoTriedKey = null
            return
        }
        usbHost = UsbHostInfo(acc.description ?: "", acc.serial)
        val key = "${acc.description}|${acc.serial}"
        RvLog.i("accessory đang cắm: desc='${acc.description}' serial=${if (AccessoryLink.otpOf(acc) != null) "hợp lệ" else "KHÔNG hợp lệ"} auto=${settings.autoConnectUsb}")
        if (settings.autoConnectUsb && session == null && !usbStarting && autoTriedKey != key) {
            autoTriedKey = key
            startUsbDisplay()
        }
    }

    override fun onResume() {
        super.onResume()
        update.onResume()
        refreshAccessory()
    }

    /**
     * Ưu tiên: USB accessory -> ADB reverse -> USB tether/LAN -> server.
     * Thăm dò ở thread nền; hai nhánh đầu chạy phiên "Màn hình phụ (USB)".
     */
    private fun startUsbDisplay() {
        if (usbStarting || session != null) {
            RvLog.i("startUsbDisplay bỏ qua: starting=$usbStarting session=${session != null}")
            return
        }
        usbError = null
        val acc = AccessoryLink.findAccessory(this)
        val accOtp = AccessoryLink.otpOf(acc)
        RvLog.i("startUsbDisplay: accessory=${acc != null} otpTựĐộng=${accOtp != null}")
        // USB accessory: OTP lấy từ accessory.serial, không hỏi PIN. PIN chỉ còn cho ADB reverse / host cũ.
        if (accOtp == null && !StreamSettings.isValidOtp(settings.usbOtp)) {
            pinPrompt = PinPrompt(null)
            return
        }
        usbStarting = true
        Thread({
            val route = LinkSelector.select(applicationContext, UsbTetherDetector())
            RvLog.i("LinkSelector -> ${route::class.java.simpleName}")
            runOnUiThread {
                usbStarting = false
                if (session != null || isDestroyed) return@runOnUiThread
                when (route) {
                    is Route.Usb -> {
                        val otp = if (route.link is AccessoryLink) accOtp else null
                        val s = SessionController(this, settings, usbDisplay = true, otp = otp)
                        s.onLinkClosed = { reason ->
                            runOnUiThread {
                                if (session === s) {
                                    // Lỗi: thử gửi log trước (best-effort, tối đa 2 s) nếu handle native còn hợp lệ.
                                    sendLogThenEnd(s, if (reason != null) 2000L else 0L) {
                                    endSession(); screen = Screen.HOME
                                    usbError = reason
                                    // Rút cáp -> thẻ biến mất; cắm lại sẽ tự kết nối lại.
                                    autoTriedKey = null
                                    }
                                }
                            }
                        }
                        s.onNativeFailed = { msg ->
                            runOnUiThread {
                                if (session === s) {
                                    sendLogThenEnd(s) {
                                    endSession(); screen = Screen.HOME
                                    if (otp != null) usbError = msg ?: "ghép đôi thất bại"
                                    else pinPrompt = PinPrompt(msg ?: "")
                                    }
                                }
                            }
                        }
                        session = s
                        acquireSessionWake()
                        s.attachLink(route.link)
                        screen = Screen.SESSION
                    }
                    // Tether/LAN/server: dùng đường kết nối LAN (UDP trực tiếp); phiên vẫn ở chế độ màn hình phụ.
                    is Route.Tether -> connectTo(
                        route.localAddress,
                        usbDisplay = true,
                        otp = settings.usbOtp.takeIf { StreamSettings.isValidOtp(it) },
                    )
                    Route.Server -> { usbError = "không tìm thấy đường kết nối USB" }
                }
            }
        }, "rv-link-select").also { it.isDaemon = true; it.start() }
    }

    private fun connectTo(target: String, usbDisplay: Boolean = false, otp: String? = null) {
        if (session != null) {
            RvLog.i("connectTo($target) bỏ qua: đã có phiên đang chạy")
            return
        }
        val pin = otp ?: settings.usbOtp.takeIf { StreamSettings.isValidOtp(it) }
        if (pin == null) {
            pinPrompt = PinPrompt(error = null, target = target, usbDisplay = usbDisplay)
            return
        }
        val s = SessionController(this, settings, usbDisplay = usbDisplay, otp = pin)
        s.onNativeFailed = { msg ->
            runOnUiThread {
                if (session === s) {
                    sendLogThenEnd(s) {
                        endSession()
                        screen = Screen.HOME
                        pinPrompt = PinPrompt(
                            error = msg ?: "kết nối thất bại",
                            target = target,
                            usbDisplay = usbDisplay,
                        )
                    }
                }
            }
        }
        session = s
        acquireSessionWake()
        s.connectLan(target)
        if (session === s) {
            screen = Screen.SESSION
        }
    }

    private fun rememberRecent(target: String) {
        val name = lanHosts.firstOrNull { it.address == target }?.name ?: target
        val list = recentRepo.add(name, target)
        recents.clear()
        recents.addAll(list.map { LanHost(it.first, it.second) })
    }

    @Composable
    private fun App() {
        val widthClass = rememberWidthClass()
        UpdateDialog(update)
        when (screen) {
            Screen.HOME -> AppShell(
                selected = 0,
                onSelect = { if (it == 1) screen = Screen.SETTINGS },
                widthClass = widthClass,
            ) { padding ->
                HomeScreen(
                    lanHosts = lanHosts,
                    recentHosts = recents,
                    onConnect = { target ->
                        pinPrompt = PinPrompt(error = null, target = target, usbDisplay = false)
                    },
                    widthClass = widthClass,
                    update = update,
                    contentPadding = padding,
                    usbOtp = settings.usbOtp,
                    pinPrompt = pinPrompt,
                    onUsbDisplay = { pinPrompt = PinPrompt(null) },
                    onPinSubmit = { otp ->
                        val prompt = pinPrompt
                        pinPrompt = null
                        settings = settings.copy(usbOtp = otp).also { repo.save(it) }
                        val target = prompt?.target
                        if (prompt != null && target != null) {
                            rememberRecent(target)
                            connectTo(target, usbDisplay = prompt.usbDisplay, otp = otp)
                        } else {
                            startUsbDisplay()
                        }
                    },
                    onPinDismiss = { pinPrompt = null },
                    usbHost = usbHost,
                    usbBusy = usbStarting,
                    usbError = usbError,
                    onUsbConnect = { startUsbDisplay() },
                    onLog = { screen = Screen.LOG },
                    onDiagnostics = { screen = Screen.DIAG },
                    onBench = { screen = Screen.BENCH },
                )
            }
            Screen.LOG -> {
                BackHandler { screen = Screen.HOME }
                LogScreen(
                    onBack = { screen = Screen.HOME },
                    sessionHandle = { session?.takeIf { it.isConnected }?.nativeHandle ?: 0L },
                )
            }
            Screen.BENCH -> {
                BackHandler { screen = Screen.HOME }
                BenchScreen(onBack = { screen = Screen.HOME })
            }
            Screen.DIAG -> {
                BackHandler { screen = Screen.HOME }
                DiagnosticsScreen(onBack = { screen = Screen.HOME })
            }
            Screen.SETTINGS -> {
                BackHandler { screen = Screen.HOME }
                AppShell(
                    selected = 1,
                    onSelect = { if (it == 0) screen = Screen.HOME },
                    widthClass = widthClass,
                ) { padding ->
                    SettingsScreen(
                        settings = settings,
                        onChange = { settings = it; repo.save(it) },
                        update = update,
                        widthClass = widthClass,
                        contentPadding = padding,
                    )
                }
            }
            Screen.SESSION -> {
                val s = session
                if (s == null) {
                    screen = Screen.HOME
                } else {
                    BackHandler { endSession(); screen = Screen.HOME }
                    SessionScreen(
                        s,
                        showModifierBar = !s.usbDisplay || settings.usbShowModifierBar,
                        debugEnabled = settings.debugOverlay,
                        showFpsHud = settings.showFpsHud,
                        onFpsHudChanged = { on -> settings = settings.copy(showFpsHud = on).also { repo.save(it) } },
                    )
                }
            }
        }
    }

    /** Phiên lỗi: gửi log lên máy tính (nếu bật) rồi mới đóng phiên; tối đa chờ 3 giây. */
    private fun sendLogThenEnd(s: SessionController, timeoutMs: Long = 3000L, then: () -> Unit) {
        val h = s.nativeHandle
        if (timeoutMs <= 0L || !settings.autoSendLogOnError || h == 0L || !NativeBridge.available) { then(); return }
        val done = java.util.concurrent.atomic.AtomicBoolean(false)
        val finish = { if (done.compareAndSet(false, true)) then() }
        Thread({
            LogShare.upload(h)
            runOnUiThread { finish() }
        }, "rv-log-autosend").also { it.isDaemon = true; it.start() }
        window.decorView.postDelayed({ finish() }, timeoutMs)
    }

    private fun endSession() {
        RvLog.i("endSession")
        session?.let {
            if (NativeBridge.available && it.nativeHandle != 0L) NativeBridge.nativeDisconnect(it.nativeHandle)
            it.release()
        }
        session = null
        releaseSessionWake()
    }

    private fun acquireSessionWake() {
        try {
            var wl = sessionWake
            if (wl == null) {
                val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
                wl = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "rv:session")
                wl.setReferenceCounted(false)
                sessionWake = wl
            }
            if (!wl.isHeld) wl.acquire()
        } catch (e: Exception) {
            RvLog.w("không lấy được WAKE_LOCK", e)
        }
    }

    private fun releaseSessionWake() {
        try {
            val wl = sessionWake
            if (wl != null && wl.isHeld) wl.release()
        } catch (e: Exception) {
            RvLog.w("nhả WAKE_LOCK lỗi", e)
        }
    }

    override fun onStart() {
        super.onStart()
        locks.acquire()
        watcher.start()
        if (!detachRegistered) {
            ContextCompat.registerReceiver(
                this, detachReceiver, IntentFilter(UsbManager.ACTION_USB_ACCESSORY_DETACHED), ContextCompat.RECEIVER_NOT_EXPORTED,
            )
            detachRegistered = true
        }
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        if (NativeBridge.available) {
            NativeBridge.nativeStartDiscovery()
            session?.let { if (it.nativeHandle != 0L) NativeBridge.nativePauseVideo(it.nativeHandle, false) }
        }
        session?.onAppForeground()
    }

    override fun onStop() {
        // Âm thanh dừng khi ra nền (WAKE_LOCK vẫn giữ để phiên không bị ngắt).
        session?.onAppBackground()
        // Gửi PauseVideo để host dừng chụp/mã hóa khi app ra nền.
        if (NativeBridge.available) {
            session?.let { if (it.nativeHandle != 0L) NativeBridge.nativePauseVideo(it.nativeHandle, true) }
            NativeBridge.nativeStopDiscovery()
        }
        if (detachRegistered) {
            detachRegistered = false
            try { unregisterReceiver(detachReceiver) } catch (_: Exception) {}
        }
        watcher.stop()
        locks.release()
        super.onStop()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        session?.onWindowFocusChanged(hasFocus)
    }

    override fun onDestroy() {
        update.release()
        uiScope.cancel()
        endSession()
        NativeBridge.callbacks = null
        super.onDestroy()
    }

    private fun requestNetworkPermissions() {
        val perms = mutableListOf<String>()
        if (Build.VERSION.SDK_INT >= 33) perms += Manifest.permission.NEARBY_WIFI_DEVICES
        // Android 16+: quyền mạng cục bộ. Tên chuỗi chưa xác minh với SDK 36 thật.
        if (Build.VERSION.SDK_INT >= 36) perms += "android.permission.ACCESS_LOCAL_NETWORK"
        if (perms.isNotEmpty()) {
            try { permissionLauncher.launch(perms.toTypedArray()) } catch (_: Exception) {}
        }
    }
}
