package com.remoteview.client

import android.Manifest
import android.content.Intent
import android.hardware.usb.UsbManager
import android.os.Build
import android.os.Bundle
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
import com.remoteview.client.net.LanLocks
import com.remoteview.client.net.NetworkWatcher
import com.remoteview.client.net.UsbTetherDetector
import com.remoteview.client.session.SessionController
import com.remoteview.client.usb.LinkSelector
import com.remoteview.client.usb.Route
import com.remoteview.client.ui.HomeScreen
import com.remoteview.client.ui.LanHost
import com.remoteview.client.ui.RemoteViewTheme
import com.remoteview.client.ui.SessionScreen
import com.remoteview.client.ui.SettingsScreen

// UNBUILT: chưa biên dịch.

internal enum class Screen { HOME, SETTINGS, SESSION }

class MainActivity : ComponentActivity() {

    private lateinit var locks: LanLocks
    private lateinit var watcher: NetworkWatcher
    private lateinit var repo: SettingsRepository
    private var session: SessionController? = null
    private val lanHosts = mutableStateListOf<LanHost>()
    private var screen by mutableStateOf(Screen.HOME)
    private var settings by mutableStateOf(StreamSettings())
    private var usbStarting = false

    private val permissionLauncher =
        registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { /* kết quả: bỏ qua, mạng vẫn chạy */ }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        locks = LanLocks(this)
        watcher = NetworkWatcher(this)
        repo = SettingsRepository(this)
        settings = repo.load()
        requestNetworkPermissions()

        NativeBridge.callbacks = object : NativeBridge.Callbacks {
            override fun onVideoFrame(data: ByteArray, ptsUs: Long, flags: Int) {
                session?.submitFrame(data, ptsUs, flags)
            }
            override fun onConnectionState(state: Int, message: String?) {}
            override fun onHostDiscovered(name: String, address: String) {
                runOnUiThread { if (lanHosts.none { it.address == address }) lanHosts.add(LanHost(name, address)) }
            }
            override fun onStats(rttMs: Float, bitrateKbps: Int, lossPercent: Float) {}
        }

        setContent {
            RemoteViewTheme { App() }
        }
        // "Cắm là hiện": mở app bằng intent USB_ACCESSORY_ATTACHED -> tự vào phiên màn hình phụ.
        handleUsbIntent(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleUsbIntent(intent)
    }

    private fun handleUsbIntent(intent: Intent?) {
        if (intent?.action == UsbManager.ACTION_USB_ACCESSORY_ATTACHED) startUsbDisplay()
    }

    /**
     * Ưu tiên: USB accessory -> ADB reverse -> USB tether/LAN -> server.
     * Thăm dò ở thread nền; hai nhánh đầu chạy phiên "Màn hình phụ (USB)".
     */
    private fun startUsbDisplay() {
        if (usbStarting || session != null) return
        usbStarting = true
        Thread({
            val route = LinkSelector.select(applicationContext, UsbTetherDetector())
            runOnUiThread {
                usbStarting = false
                if (session != null || isDestroyed) return@runOnUiThread
                when (route) {
                    is Route.Usb -> {
                        val s = SessionController(this, settings, usbDisplay = true)
                        s.onLinkClosed = { runOnUiThread { if (session === s) { endSession(); screen = Screen.HOME } } }
                        session = s
                        s.attachLink(route.link)
                        screen = Screen.SESSION
                    }
                    // Tether/LAN/server: dùng đường kết nối thường (Rust tự tìm host); phiên vẫn ở chế độ màn hình phụ.
                    is Route.Tether -> connectTo(route.localAddress, usbDisplay = true)
                    Route.Server -> {} // chưa có gì để tự kết nối: ở lại màn hình chính
                }
            }
        }, "rv-link-select").also { it.isDaemon = true; it.start() }
    }

    private fun connectTo(target: String, usbDisplay: Boolean = false) {
        val s = SessionController(this, settings, usbDisplay)
        session = s
        if (NativeBridge.available) {
            // TODO(rust-core): cấu hình JSON theo thỏa thuận giao thức; caps nằm trong helloJson.
            s.nativeHandle = NativeBridge.nativeConnect(target, s.helloJson)
        }
        screen = Screen.SESSION
    }

    @Composable
    private fun App() {
        when (screen) {
            Screen.HOME -> HomeScreen(
                lanHosts = lanHosts,
                onConnect = { target -> connectTo(target) },
                onOpenSettings = { screen = Screen.SETTINGS },
                onUsbDisplay = { startUsbDisplay() },
            )
            Screen.SETTINGS -> {
                BackHandler { screen = Screen.HOME }
                SettingsScreen(
                    settings = settings,
                    onChange = { settings = it; repo.save(it) },
                    onBack = { screen = Screen.HOME },
                )
            }
            Screen.SESSION -> {
                val s = session
                if (s == null) {
                    screen = Screen.HOME
                } else {
                    BackHandler { endSession(); screen = Screen.HOME }
                    SessionScreen(s, showModifierBar = !s.usbDisplay || settings.usbShowModifierBar)
                }
            }
        }
    }

    private fun endSession() {
        session?.let {
            if (NativeBridge.available && it.nativeHandle != 0L) NativeBridge.nativeDisconnect(it.nativeHandle)
            it.release()
        }
        session = null
    }

    override fun onStart() {
        super.onStart()
        locks.acquire()
        watcher.start()
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        if (NativeBridge.available) {
            NativeBridge.nativeStartDiscovery()
            session?.let { NativeBridge.nativePauseVideo(it.nativeHandle, false) }
        }
    }

    override fun onStop() {
        // Gửi PauseVideo để host dừng chụp/mã hóa khi app ra nền.
        if (NativeBridge.available) {
            session?.let { NativeBridge.nativePauseVideo(it.nativeHandle, true) }
            NativeBridge.nativeStopDiscovery()
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
