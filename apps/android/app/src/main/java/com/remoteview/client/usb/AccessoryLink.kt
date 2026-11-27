package com.remoteview.client.usb

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.hardware.usb.UsbAccessory
import android.hardware.usb.UsbManager
import android.os.Build
import android.os.Handler
import android.os.Looper
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
class AccessoryLink(
    context: Context,
    private val initialAccessory: UsbAccessory? = null,
) : FramedStreamLink("aoa") {

    private val ctx = context.applicationContext
    private val usb = ctx.getSystemService(Context.USB_SERVICE) as UsbManager
    private val fdLock = Any()
    private val mainHandler = Handler(Looper.getMainLooper())
    private var pfd: ParcelFileDescriptor? = null
    @Volatile private var openThread: Thread? = null
    @Volatile private var accessory: UsbAccessory? = null
    private var receiverRegistered = false

    private val permissionTimeout = Runnable {
        if (!isClosed && !permissionGranted) {
            RvLog.w("AOA: quá thời gian chờ cấp quyền USB (${PERMISSION_TIMEOUT_MS}ms)")
            fail("quá thời gian chờ quyền USB accessory")
        }
    }

    private val receiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            when (intent.action) {
                ACTION_PERMISSION -> {
                    cancelPermissionTimeout()
                    val acc = accessoryFrom(intent) ?: accessory
                    val granted = intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)
                    RvLog.i("AOA: kết quả xin quyền granted=$granted acc=${acc != null}")
                    if (acc != null && granted) {
                        permissionGranted = true
                        openAccessory(acc)
                    } else {
                        permissionDenied = true
                        fail("chưa được cấp quyền USB accessory")
                    }
                }
                UsbManager.ACTION_USB_ACCESSORY_DETACHED -> {
                    val acc = accessoryFrom(intent)
                    val cur = accessory
                    if (cur == null || acc == null || acc == cur) {
                        detachedByCable = true
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
    @Volatile var permissionDenied = false
        private set
    @Volatile var fdOpened = false
        private set
    /** true CHỈ khi nhận ACTION_USB_ACCESSORY_DETACHED (rút cáp vật lý), để phân biệt với lỗi I/O / host restart. */
    @Volatile var detachedByCable = false
        private set

    override fun start() {
        val acc = initialAccessory ?: findAccessory(ctx) ?: run {
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
                if (!isClosed) {
                    mainHandler.removeCallbacks(permissionTimeout)
                    mainHandler.postDelayed(permissionTimeout, PERMISSION_TIMEOUT_MS)
                }
                usb.requestPermission(acc, pi)
            } catch (e: Exception) {
                cancelPermissionTimeout()
                RvLog.e("AOA: requestPermission lỗi", e)
                fail(e.message)
            }
        }
    }

    private fun cancelPermissionTimeout() {
        mainHandler.removeCallbacks(permissionTimeout)
    }

    private fun openAccessory(acc: UsbAccessory) {
        val t = Thread({
            try {
                var fd: ParcelFileDescriptor? = null
                for (attempt in 0..OPEN_RETRY_DELAYS_MS.size) {
                    if (isClosed) return@Thread
                    fd = usb.openAccessory(acc)
                    if (fd != null) break
                    if (attempt < OPEN_RETRY_DELAYS_MS.size) {
                        val delayMs = OPEN_RETRY_DELAYS_MS[attempt]
                        RvLog.w("AOA: openAccessory trả về null (lần ${attempt + 1}), thử lại sau ${delayMs}ms")
                        try {
                            Thread.sleep(delayMs)
                        } catch (_: InterruptedException) {
                            return@Thread
                        }
                    }
                }
                if (fd == null) {
                    if (!isClosed) throw IllegalStateException("openAccessory trả về null")
                    return@Thread
                }
                synchronized(fdLock) {
                    if (isClosed) {
                        RvLog.w("AOA: link đã đóng trước khi gán pfd -> đóng fd=${fd.fd} ngay")
                        runCatching { fd.close() }
                        return@Thread
                    }
                    pfd = fd
                }
                fdOpened = true
                RvLog.i("AOA: openAccessory OK (fd=${fd.fd})")
                val fileDesc = fd.fileDescriptor
                runStreams(FileInputStream(fileDesc), FileOutputStream(fileDesc))
            } catch (e: Exception) {
                if (!isClosed) {
                    RvLog.e("AOA: mở accessory lỗi", e)
                    fail(e.message)
                }
            }
        }, "rv-aoa-open").also { it.isDaemon = true }
        openThread = t
        t.start()
    }

    private fun registerReceiver() {
        synchronized(fdLock) {
            if (receiverRegistered || isClosed) return
            val f = IntentFilter().apply {
                addAction(ACTION_PERMISSION)
                addAction(UsbManager.ACTION_USB_ACCESSORY_DETACHED)
            }
            // Android 13+: phải khai báo cờ. Hệ thống gửi kết quả quyền từ tiến trình system_server nên dùng EXPORTED
            // (intent đã giới hạn theo package của app; kết quả giả cũng không mở được accessory nếu chưa có quyền thật).
            ContextCompat.registerReceiver(ctx, receiver, f, ContextCompat.RECEIVER_EXPORTED)
            receiverRegistered = true
        }
        RvLog.i("AOA: đã đăng ký receiver (API ${Build.VERSION.SDK_INT})")
    }

    override fun closeResources() {
        cancelPermissionTimeout()
        val ot = openThread
        openThread = null
        if (ot != null && ot !== Thread.currentThread()) {
            ot.interrupt()
        }
        val toClose: ParcelFileDescriptor?
        val unregister: Boolean
        synchronized(fdLock) {
            unregister = receiverRegistered
            receiverRegistered = false
            toClose = pfd
            pfd = null
        }
        if (unregister) {
            try { ctx.unregisterReceiver(receiver) } catch (_: Exception) {}
        }
        try { toClose?.close() } catch (_: Exception) {}
    }

    @Suppress("DEPRECATION")
    private fun accessoryFrom(intent: Intent): UsbAccessory? =
        if (Build.VERSION.SDK_INT >= 33) intent.getParcelableExtra(UsbManager.EXTRA_ACCESSORY, UsbAccessory::class.java)
        else intent.getParcelableExtra(UsbManager.EXTRA_ACCESSORY)

    companion object {
        const val ACTION_PERMISSION = "com.remoteview.client.USB_ACCESSORY_PERMISSION"
        const val MANUFACTURER = "RemoteView"
        const val MODEL = "RemoteViewHost"
        private const val PERMISSION_TIMEOUT_MS = 12_000L
        private val OPEN_RETRY_DELAYS_MS = longArrayOf(150L, 200L, 300L, 400L, 500L, 500L, 500L, 500L)
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
