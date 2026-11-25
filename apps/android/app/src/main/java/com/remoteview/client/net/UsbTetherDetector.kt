package com.remoteview.client.net

import java.net.Inet4Address
import java.net.NetworkInterface

// UNBUILT: chưa biên dịch.

data class UsbTetherInfo(
    val interfaceName: String,
    /** IPv4 của điện thoại trên liên kết USB (thường 192.168.42.x / 192.168.x.x). */
    val localAddress: String,
    val prefixLength: Int,
)

/** Đường LAN theo thứ tự ưu tiên: USB tether (trễ thấp, ổn định nhất) > Wi-Fi/Ethernet > di động. */
enum class LanPath { USB_TETHER, WIFI, ETHERNET, CELLULAR }

/**
 * Phát hiện giao diện USB tethering (rndis0 / usb0 / ncm0) bằng NetworkInterface.
 * USB tethering = đường LAN ưu tiên cao nhất. Việc tìm máy Windows ở đầu kia
 * (quét subnet / mDNS) do lõi Rust đảm nhiệm.
 */
class UsbTetherDetector {

    private val nameRegex = Regex("^(rndis|usb|ncm)\\d+$", RegexOption.IGNORE_CASE)

    fun detect(): UsbTetherInfo? {
        val ifaces = try {
            NetworkInterface.getNetworkInterfaces()?.toList().orEmpty()
        } catch (_: Exception) {
            return null
        }
        for (ni in ifaces) {
            val up = try { ni.isUp && !ni.isLoopback } catch (_: Exception) { false }
            if (!up || !nameRegex.matches(ni.name)) continue
            val addr = ni.interfaceAddresses.firstOrNull { it.address is Inet4Address } ?: continue
            return UsbTetherInfo(ni.name, addr.address.hostAddress.orEmpty(), addr.networkPrefixLength.toInt())
        }
        return null
    }
}
