package com.remoteview.client.net

import android.annotation.SuppressLint
import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

// UNBUILT: chưa biên dịch.

data class NetState(
    val transport: LanPath? = null,
    val wifiRssiDbm: Int? = null,
    /** Mạng di động đã được pre-warm (requestNetwork) và sẵn sàng làm đường dự phòng. */
    val cellularWarm: Boolean = false,
    val usbTether: UsbTetherInfo? = null,
) {
    /** Đường tốt nhất: USB tether > mạng mặc định (Wi-Fi/Ethernet) > di động. */
    val bestPath: LanPath?
        get() = when {
            usbTether != null -> LanPath.USB_TETHER
            transport == LanPath.WIFI || transport == LanPath.ETHERNET -> transport
            cellularWarm || transport == LanPath.CELLULAR -> LanPath.CELLULAR
            else -> null
        }
}

/**
 * Theo dõi mạng mặc định; khi Wi-Fi yếu (RSSI < -75 dBm) pre-warm mạng di động bằng requestNetwork
 * để chuyển đường không phải chờ bật radio. [cellularNetwork] để lõi Rust bindSocket sau này.
 */
class NetworkWatcher(
    context: Context,
    private val usbDetector: UsbTetherDetector = UsbTetherDetector(),
) {
    private val appCtx = context.applicationContext
    private val cm = appCtx.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    private val wifi = appCtx.getSystemService(Context.WIFI_SERVICE) as WifiManager
    private val handler = Handler(Looper.getMainLooper())

    private val _state = MutableStateFlow(NetState())
    val state: StateFlow<NetState> = _state.asStateFlow()

    @Volatile var cellularNetwork: Network? = null
        private set

    private var started = false
    private var cellularRequested = false

    private val defaultCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
            val path = when {
                caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> LanPath.WIFI
                caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> LanPath.ETHERNET
                caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> LanPath.CELLULAR
                else -> null
            }
            val rssi = if (path == LanPath.WIFI) readRssi(caps) else null
            _state.update { it.copy(transport = path, wifiRssiDbm = rssi) }
            evaluateCellular()
            refreshUsb()
        }

        override fun onLost(network: Network) {
            _state.update { it.copy(transport = null, wifiRssiDbm = null) }
            evaluateCellular()
            refreshUsb()
        }
    }

    private val cellularCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            cellularNetwork = network
            _state.update { it.copy(cellularWarm = true) }
        }

        override fun onLost(network: Network) {
            if (cellularNetwork == network) cellularNetwork = null
            _state.update { it.copy(cellularWarm = false) }
        }

        override fun onUnavailable() {
            cellularRequested = false
            _state.update { it.copy(cellularWarm = false) }
        }
    }

    private val poll = object : Runnable {
        override fun run() {
            if (!started) return
            val s = _state.value
            if (s.transport == LanPath.WIFI) {
                @Suppress("DEPRECATION")
                val rssi = try { wifi.connectionInfo?.rssi } catch (_: Exception) { null }
                if (rssi != null && rssi != -127) _state.update { it.copy(wifiRssiDbm = rssi) }
                evaluateCellular()
            }
            refreshUsb()
            handler.postDelayed(this, POLL_MS)
        }
    }

    @SuppressLint("MissingPermission")
    fun start() {
        if (started) return
        started = true
        try {
            cm.registerDefaultNetworkCallback(defaultCallback, handler)
        } catch (e: Exception) {
            Log.w(TAG, "registerDefaultNetworkCallback: $e")
        }
        handler.post(poll)
    }

    fun stop() {
        if (!started) return
        started = false
        handler.removeCallbacks(poll)
        try { cm.unregisterNetworkCallback(defaultCallback) } catch (_: Exception) {}
        releaseCellular()
    }

    private fun readRssi(caps: NetworkCapabilities): Int? {
        if (Build.VERSION.SDK_INT >= 29) {
            val s = caps.signalStrength
            if (s != NetworkCapabilities.SIGNAL_STRENGTH_UNSPECIFIED) return s
        }
        @Suppress("DEPRECATION")
        return try { wifi.connectionInfo?.rssi?.takeIf { it != -127 } } catch (_: Exception) { null }
    }

    private fun refreshUsb() {
        val usb = usbDetector.detect()
        if (usb != _state.value.usbTether) _state.update { it.copy(usbTether = usb) }
    }

    private fun evaluateCellular() {
        val s = _state.value
        val weakWifi = s.transport == LanPath.WIFI && (s.wifiRssiDbm ?: 0) < WEAK_RSSI_DBM
        val strongAgain = s.transport != LanPath.WIFI || (s.wifiRssiDbm ?: -100) > WEAK_RSSI_DBM + 5
        if (weakWifi && !cellularRequested) requestCellular()
        else if (strongAgain && cellularRequested) releaseCellular()
    }

    @SuppressLint("MissingPermission") // CHANGE_NETWORK_STATE đã khai báo trong manifest
    private fun requestCellular() {
        cellularRequested = true
        val req = NetworkRequest.Builder()
            .addTransportType(NetworkCapabilities.TRANSPORT_CELLULAR)
            .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .build()
        try {
            cm.requestNetwork(req, cellularCallback, handler)
        } catch (e: Exception) {
            cellularRequested = false
            Log.w(TAG, "requestNetwork(cellular): $e")
        }
    }

    private fun releaseCellular() {
        if (!cellularRequested) return
        cellularRequested = false
        try { cm.unregisterNetworkCallback(cellularCallback) } catch (_: Exception) {}
        cellularNetwork = null
        _state.update { it.copy(cellularWarm = false) }
    }

    companion object {
        private const val TAG = "RvNetWatcher"
        const val WEAK_RSSI_DBM = -75
        private const val POLL_MS = 3_000L
    }
}
