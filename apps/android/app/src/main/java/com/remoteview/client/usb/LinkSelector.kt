package com.remoteview.client.usb

import android.content.Context
import android.hardware.usb.UsbAccessory
import com.remoteview.client.net.UsbTetherDetector

// UNBUILT: chưa biên dịch.

/** Đường kết nối được chọn. */
sealed interface Route {
    /** Liên kết datagram qua USB (AOA hoặc ADB reverse); dữ liệu đi qua NativeBridge. */
    data class Usb(val link: DatagramLink) : Route
    /** USB tethering / LAN: [target] = địa chỉ host (do lõi Rust tìm trong subnet tether). */
    data class Tether(val localAddress: String) : Route
    /** Còn lại: ID/địa chỉ do người dùng nhập, đi qua server/LAN. */
    data object Server : Route
}

/**
 * Thứ tự ưu tiên: USB accessory (đã cắm) -> ADB reverse -> USB tether/LAN -> server.
 * [select] có thể thăm dò socket nên phải gọi ở thread nền.
 */
object LinkSelector {
    fun select(
        context: Context,
        tether: UsbTetherDetector = UsbTetherDetector(),
        preferredAccessory: UsbAccessory? = null,
    ): Route {
        val acc = preferredAccessory ?: AccessoryLink.findAccessory(context)
        if (acc != null) return Route.Usb(AccessoryLink(context, acc))
        if (AdbReverseLink.probe()) return Route.Usb(AdbReverseLink())
        tether.detect()?.let { return Route.Tether(it.localAddress) }
        return Route.Server
    }
}
