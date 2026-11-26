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
import androidx.core.content.ContextCompat
import com.remoteview.client.RvLog
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
                    RvLog.i("AOA: kết quả xin quyền granted=$granted acc=${acc != null}")
                    if (acc != null && granted) { permissionGranted = true; openAccessory(acc) }
                    else fail("chưa được cấp quyền USB accessory")
                }
                UsbManager.ACTION_USB_ACCESSORY_DETACHED -> {
                    val acc = accessoryFrom(intent)
                    val cur = accessory
                    if (cur == null || acc == null || acc == cur) {
                        RvLog.w("AOA: accessory bị rút")
                        fail("đã rút cáp USB")
                    }
                }
            }
        }
    }

    /** Số serial (token ghép đôi 8 chữ số do host đặt) của accessory đang dùng; null nếu chưa biết. */
    val serial: String? get() = accessory?.serial

    @Volatile var permissionGranted = false
        private set
    @Volatile var fdOpened = false
        private set

    override fun start() {
        val acc = findAccessory(ctx) ?: run {
            RvLog.e("AOA: không thấy accessory RemoteView trong accessoryList")
            fail("không có accessory"); return
        }
        accessory = acc
        RvLog.i("AOA: tìm thấy accessory '${acc.manufacturer}/${acc.model}' desc='${acc.description}' ver='${acc.version}'")
        registerReceiver()
        if (usb.hasPermission(acc)) {
            permissionGranted = true
            RvLog.i("AOA: đã có quyền, mở accessory")
            openAccessory(acc)
        } else {
            // Android 12+ bắt buộc FLAG_MUTABLE cho PendingIntent của requestPermission (hệ thống điền extras).
            // Intent phải có package (explicit) để hợp lệ với targetSdk 34+.
            val flags = PendingIntent.FLAG_UPDATE_CURRENT or
                (if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0)
            val pi = PendingIntent.getBroadcast(
                ctx, 0, Intent(ACTION_PERMISSION).setPackage(ctx.packageName), flags,
            )
            RvLog.i("AOA: xin quyền USB accessory")
            try {
                usb.requestPermission(acc, pi)
            } catch (e: Exception) {
                RvLog.e("AOA: requestPermission lỗi", e)
                fail(e.message)
            }
        }
    }

    private fun openAccessory(acc: UsbAccessory) {
        Thread({
            try {
                val fd = usb.openAccessory(acc) ?: throw IllegalStateException("openAccessory trả về null")
                pfd = fd
                fdOpened = true
                RvLog.i("AOA: openAccessory OK (fd=${fd.fd})")
                val fileDesc = fd.fileDescriptor
                runStreams(FileInputStream(fileDesc), FileOutputStream(fileDesc))
            } catch (e: Exception) {
                RvLog.e("AOA: mở accessory lỗi", e)
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
        // Android 13+: phải khai báo cờ. Hệ thống gửi kết quả quyền từ tiến trình system_server nên dùng EXPORTED
        // (intent đã giới hạn theo package của app; kết quả giả cũng không mở được accessory nếu chưa có quyền thật).
        ContextCompat.registerReceiver(ctx, receiver, f, ContextCompat.RECEIVER_EXPORTED)
        receiverRegistered = true
        RvLog.i("AOA: đã đăng ký receiver (API ${Build.VERSION.SDK_INT})")
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
        const val ACTION_PERMISSION = "com.remoteview.client.USB_ACCESSORY_PERMISSION"
        const val MANUFACTURER = "RemoteView"
        const val MODEL = "RemoteViewHost"
        private val SERIAL_RE = Regex("^\\d{6,9}$")

        /** Accessory của Remote View đang cắm (nếu có). */
        fun findAccessory(context: Context): UsbAccessory? {
            val m = context.getSystemService(Context.USB_SERVICE) as? UsbManager ?: return null
            return try {
                m.accessoryList?.firstOrNull { it.manufacturer == MANUFACTURER && it.model == MODEL }
            } catch (e: Exception) {
                RvLog.e("AOA: đọc accessoryList lỗi", e)
                null
            }
        }

        fun isAttached(context: Context): Boolean = findAccessory(context) != null

        /** Token ghép đôi hợp lệ (6-9 chữ số) trong accessory.serial, dùng làm OTP cho USB. */
        fun otpOf(acc: UsbAccessory?): String? = acc?.serial?.trim()?.takeIf { SERIAL_RE.matches(it) }
    }
}
