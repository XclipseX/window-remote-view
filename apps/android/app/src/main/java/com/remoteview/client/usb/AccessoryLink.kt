package com.remoteview.client.usb

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.hardware.usb.UsbAccessory
import android.hardware.usb.UsbManager
import android.os.Build
import android.os.ParcelFileDescriptor
import android.util.Log
import androidx.core.content.ContextCompat
import java.io.FileInputStream
import java.io.FileOutputStream

// UNBUILT: chưa biên dịch.

/**
 * Android Open Accessory (phía accessory = điện thoại). Host Windows đưa điện thoại vào chế độ AOA
 * (manufacturer "RemoteView", model "RemoteViewHost"), Android phát USB_ACCESSORY_ATTACHED.
 * Không cần bật tùy chọn nhà phát triển / gỡ lỗi USB.
 *
 * Khi activity được mở bởi intent ATTACHED, quyền được cấp sẵn; nếu app đang chạy sẵn và accessory
 * cắm sau, ta tự xin quyền qua [UsbManager.requestPermission].
 */
class AccessoryLink(context: Context) : FramedStreamLink("aoa") {

    private val ctx = context.applicationContext
    private val usb = ctx.getSystemService(Context.USB_SERVICE) as UsbManager
    private var pfd: ParcelFileDescriptor? = null
    @Volatile private var accessory: UsbAccessory? = null
    private var receiverRegistered = false

    private val receiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            when (intent.action) {
                ACTION_PERMISSION -> {
                    val acc = accessoryFrom(intent) ?: accessory
                    val granted = intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)
                    if (acc != null && granted) openAccessory(acc) else fail("chưa được cấp quyền USB accessory")
                }
                UsbManager.ACTION_USB_ACCESSORY_DETACHED -> {
                    val acc = accessoryFrom(intent)
                    val cur = accessory
                    if (cur == null || acc == null || acc == cur) fail("đã rút cáp USB")
                }
            }
        }
    }

    override fun start() {
        val acc = findAccessory(ctx) ?: run { fail("không có accessory"); return }
        accessory = acc
        registerReceiver()
        if (usb.hasPermission(acc)) {
            openAccessory(acc)
        } else {
            val flags = PendingIntent.FLAG_UPDATE_CURRENT or
                (if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0)
            val pi = PendingIntent.getBroadcast(
                ctx, 0, Intent(ACTION_PERMISSION).setPackage(ctx.packageName), flags,
            )
            usb.requestPermission(acc, pi)
        }
    }

    private fun openAccessory(acc: UsbAccessory) {
        Thread({
            try {
                val fd = usb.openAccessory(acc) ?: throw IllegalStateException("openAccessory trả về null")
                pfd = fd
                val fileDesc = fd.fileDescriptor
                runStreams(FileInputStream(fileDesc), FileOutputStream(fileDesc))
            } catch (e: Exception) {
                Log.w(TAG, "mở accessory lỗi: ${e.message}")
                fail(e.message)
            }
        }, "rv-aoa-open").also { it.isDaemon = true; it.start() }
    }

    private fun registerReceiver() {
        if (receiverRegistered) return
        val f = IntentFilter().apply {
            addAction(ACTION_PERMISSION)
            addAction(UsbManager.ACTION_USB_ACCESSORY_DETACHED)
        }
        ContextCompat.registerReceiver(ctx, receiver, f, ContextCompat.RECEIVER_NOT_EXPORTED)
        receiverRegistered = true
    }

    override fun closeResources() {
        if (receiverRegistered) {
            receiverRegistered = false
            try { ctx.unregisterReceiver(receiver) } catch (_: Exception) {}
        }
        try { pfd?.close() } catch (_: Exception) {}
        pfd = null
    }

    @Suppress("DEPRECATION")
    private fun accessoryFrom(intent: Intent): UsbAccessory? =
        if (Build.VERSION.SDK_INT >= 33) intent.getParcelableExtra(UsbManager.EXTRA_ACCESSORY, UsbAccessory::class.java)
        else intent.getParcelableExtra(UsbManager.EXTRA_ACCESSORY)

    companion object {
        private const val TAG = "RvAoa"
        const val ACTION_PERMISSION = "com.remoteview.client.USB_ACCESSORY_PERMISSION"
        const val MANUFACTURER = "RemoteView"
        const val MODEL = "RemoteViewHost"

        /** Accessory của Remote View đang cắm (nếu có). */
        fun findAccessory(context: Context): UsbAccessory? {
            val m = context.getSystemService(Context.USB_SERVICE) as? UsbManager ?: return null
            return m.accessoryList?.firstOrNull { it.manufacturer == MANUFACTURER && it.model == MODEL }
        }

        fun isAttached(context: Context): Boolean = findAccessory(context) != null
    }
}
