package com.remoteview.client

import android.Manifest
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
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import com.remoteview.client.net.LanLocks
import com.remoteview.client.net.NetworkWatcher
import com.remoteview.client.session.SessionController
import com.remoteview.client.ui.HomeScreen
import com.remoteview.client.ui.LanHost
import com.remoteview.client.ui.RemoteViewTheme
import com.remoteview.client.ui.SessionScreen
import com.remoteview.client.ui.SettingsScreen

// UNBUILT: chưa biên dịch.

private enum class Screen { HOME, SETTINGS, SESSION }

class MainActivity : ComponentActivity() {

    private lateinit var locks: LanLocks
    private lateinit var watcher: NetworkWatcher
    private lateinit var repo: SettingsRepository
    private var session: SessionController? = null
    private val lanHosts = mutableStateListOf<LanHost>()

    private val permissionLauncher =
        registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { /* kết quả: bỏ qua, mạng vẫn chạy */ }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        locks = LanLocks(this)
        watcher = NetworkWatcher(this)
        repo = SettingsRepository(this)
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
    }

    @Composable
    private fun App() {
        var screen by remember { mutableStateOf(Screen.HOME) }
        var settings by remember { mutableStateOf(repo.load()) }

        when (screen) {
            Screen.HOME -> HomeScreen(
                lanHosts = lanHosts,
                onConnect = { target ->
                    val s = SessionController(this@MainActivity, settings)
                    session = s
                    if (NativeBridge.available) {
                        // TODO(rust-core): cấu hình JSON (codec, fps, bitrate, ...) theo thỏa thuận giao thức.
                        s.nativeHandle = NativeBridge.nativeConnect(target, "{}")
                    }
                    screen = Screen.SESSION
                },
                onOpenSettings = { screen = Screen.SETTINGS },
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
                    SessionScreen(s)
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
