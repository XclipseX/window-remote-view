package com.remoteview.client.net

import android.content.Context
import android.net.wifi.WifiManager
import android.os.Build
import android.util.Log

// UNBUILT: chưa biên dịch.

/**
 * MulticastLock (nhận mDNS/multicast) + WifiLock FULL_LOW_LATENCY (API 29+, chỉ hiệu lực khi
 * màn hình bật và app ở tiền cảnh). Gọi [acquire] ở onStart/khi vào phiên, [release] ở onStop.
 */
class LanLocks(context: Context) {

    private val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
    private var multicast: WifiManager.MulticastLock? = null
    private var wifiLock: WifiManager.WifiLock? = null

    @Suppress("DEPRECATION")
    fun acquire() {
        try {
            if (multicast == null) {
                multicast = wifi.createMulticastLock("rv-multicast").apply { setReferenceCounted(false) }
            }
            if (multicast?.isHeld == false) multicast?.acquire()

            if (wifiLock == null) {
                val mode = if (Build.VERSION.SDK_INT >= 29) {
                    WifiManager.WIFI_MODE_FULL_LOW_LATENCY
                } else {
                    WifiManager.WIFI_MODE_FULL_HIGH_PERF
                }
                wifiLock = wifi.createWifiLock(mode, "rv-wifi").apply { setReferenceCounted(false) }
            }
            if (wifiLock?.isHeld == false) wifiLock?.acquire()
        } catch (e: Exception) {
            Log.w(TAG, "Không lấy được Wi-Fi lock: $e")
        }
    }

    fun release() {
        try { if (multicast?.isHeld == true) multicast?.release() } catch (_: Exception) {}
        try { if (wifiLock?.isHeld == true) wifiLock?.release() } catch (_: Exception) {}
    }

    private companion object { const val TAG = "RvLanLocks" }
}
