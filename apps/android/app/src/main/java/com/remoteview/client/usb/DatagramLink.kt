package com.remoteview.client.usb

import java.io.EOFException
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import com.remoteview.client.RvLog

// UNBUILT: chưa biên dịch.

/**
 * Liên kết datagram (gói tin có biên) giữa điện thoại và host qua USB.
 * Khung: u32 big-endian độ dài + payload, tối đa [FramedStreamLink.MAX_RECORD] byte.
 * Payload là gói của giao thức Remote View; lõi Rust đóng/mở gói, Kotlin chỉ chuyển byte.
 */
interface DatagramLink {
    interface Listener {
        fun onOpen() {}
        /** Gọi từ thread đọc; [data] thuộc về người nhận. */
        fun onDatagram(data: ByteArray)
        fun onClosed(reason: String?) {}
    }

    val name: String
    var listener: Listener?
    val isOpen: Boolean

    /** Bộ đếm cho lớp phủ gỡ lỗi (byte thô trên dây, gồm cả 4 byte độ dài). */
    val bytesIn: Long
    val bytesOut: Long
    val datagramsIn: Long
    val datagramsOut: Long

    /** Bắt đầu kết nối không đồng bộ; kết quả qua [Listener]. */
    fun start()

    /** Xếp hàng gửi một datagram (không chặn). false nếu liên kết đã đóng hoặc quá dài. */
    fun send(bytes: ByteArray): Boolean

    fun close()
}

/** Cài đặt chung: thread đọc + thread ghi trên một cặp stream. */
abstract class FramedStreamLink(override val name: String) : DatagramLink {

    @Volatile override var listener: DatagramLink.Listener? = null
    private val closed = AtomicBoolean(false)
    @Volatile private var opened = false
    private val queue = LinkedBlockingQueue<ByteArray>(QUEUE_LIMIT)
    private var reader: Thread? = null
    private var writer: Thread? = null

    override val isOpen: Boolean get() = opened && !closed.get()

    private val cBytesIn = AtomicLong()
    private val cBytesOut = AtomicLong()
    private val cDgIn = AtomicLong()
    private val cDgOut = AtomicLong()
    override val bytesIn: Long get() = cBytesIn.get()
    override val bytesOut: Long get() = cBytesOut.get()
    override val datagramsIn: Long get() = cDgIn.get()
    override val datagramsOut: Long get() = cDgOut.get()

    override fun send(bytes: ByteArray): Boolean {
        if (!isOpen || bytes.size > MAX_RECORD) {
            RvLog.w("[$name] bỏ datagram ${bytes.size}B: open=$isOpen")
            return false
        }
        return queue.offer(bytes)
    }

    /** Đóng tài nguyên bên dưới (socket / ParcelFileDescriptor). Phải an toàn khi gọi nhiều lần. */
    protected abstract fun closeResources()

    /** Chạy ở thread nền khi stream đã sẵn sàng. */
    protected fun runStreams(input: InputStream, output: OutputStream) {
        opened = true
        RvLog.i("[$name] stream mở, bắt đầu thread ghi và tạo phiên")
        // Thread ghi phải chạy TRƯỚC onOpen (onOpen tạo phiên native và gửi Hello ngay);
        // thread đọc chạy SAU onOpen để nativeHandle đã sẵn sàng, tránh bão gói rác (handle=0).
        writer = Thread({ writeLoop(output) }, "rv-$name-w").also { it.isDaemon = true; it.start() }
        listener?.onOpen()
        if (!closed.get()) {
            reader = Thread({ readLoop(input) }, "rv-$name-r").also { it.isDaemon = true; it.start() }
        }
    }

    /**
     * Đọc theo khối lớn rồi tự tách record. Quan trọng với AOA: driver accessory của kernel
     * bỏ phần dư của một gói bulk nếu buffer đọc nhỏ hơn gói (đọc 4 byte header sẽ làm mất dữ liệu),
     * nên luôn đọc với buffer >= 16 KiB.
     */
    private fun readLoop(input: InputStream) {
        runCatching { android.os.Process.setThreadPriority(android.os.Process.THREAD_PRIORITY_URGENT_DISPLAY) }
        val chunk = ByteArray(READ_CHUNK)
        var acc = ByteArray(READ_CHUNK * 2)
        var have = 0
        var reason: String? = null
        try {
            while (!closed.get()) {
                val r = input.read(chunk, 0, chunk.size)
                if (r < 0) throw EOFException("đầu kia đã đóng")
                if (r == 0) continue
                cBytesIn.addAndGet(r.toLong())
                if (have + r > acc.size) acc = acc.copyOf(maxOf(acc.size * 2, have + r))
                System.arraycopy(chunk, 0, acc, have, r)
                have += r
                var off = 0
                while (have - off >= 4) {
                    val len = ((acc[off].toInt() and 0xFF) shl 24) or
                        ((acc[off + 1].toInt() and 0xFF) shl 16) or
                        ((acc[off + 2].toInt() and 0xFF) shl 8) or
                        (acc[off + 3].toInt() and 0xFF)
                    if (len < 0 || len > MAX_RECORD) throw IOException("record dài bất thường: $len")
                    if (len == 0) {
                        off += 4 // bản ghi đệm tránh USB short-packet stall (512B/1024B)
                        continue
                    }
                    if (have - off - 4 < len) break
                    val payload = acc.copyOfRange(off + 4, off + 4 + len)
                    off += 4 + len
                    val n = cDgIn.incrementAndGet()
                    if (n <= 5) RvLog.i("[$name] nhận datagram #$n ${payload.size}B")
                    listener?.onDatagram(payload)
                }
                if (off > 0) {
                    System.arraycopy(acc, off, acc, 0, have - off)
                    have -= off
                }
            }
        } catch (e: Exception) {
            if (!closed.get()) reason = e.message ?: e.javaClass.simpleName
        }
        RvLog.w("[$name] thread đọc kết thúc: ${reason ?: "đóng chủ động"}")
        fail(reason)
    }

    private fun writeLoop(output: OutputStream) {
        var reason: String? = null
        try {
            while (!closed.get()) {
                val data = queue.poll(200, TimeUnit.MILLISECONDS) ?: continue
                val buf = ByteArray(4 + data.size)
                buf[0] = (data.size ushr 24).toByte()
                buf[1] = (data.size ushr 16).toByte()
                buf[2] = (data.size ushr 8).toByte()
                buf[3] = data.size.toByte()
                System.arraycopy(data, 0, buf, 4, data.size)
                output.write(buf) // một lần ghi/record để gói không bị tách trên bulk endpoint
                output.flush()
                cBytesOut.addAndGet(buf.size.toLong())
                val n = cDgOut.incrementAndGet()
                if (n <= 5) RvLog.i("[$name] đã ghi datagram #$n ${data.size}B")
            }
        } catch (e: Exception) {
            if (!closed.get()) reason = e.message ?: e.javaClass.simpleName
        }
        RvLog.w("[$name] thread ghi kết thúc: ${reason ?: "đóng chủ động"}")
        fail(reason)
    }

    /** Đóng liên kết và báo một lần duy nhất. */
    protected fun fail(reason: String?) {
        if (!closed.compareAndSet(false, true)) return
        RvLog.w("[$name] đóng liên kết: ${reason ?: "chủ động"}")
        queue.clear()
        try { closeResources() } catch (_: Exception) {}
        listener?.onClosed(reason)
    }

    override fun close() = fail(null)

    companion object {
        const val MAX_RECORD = 64 * 1024
        private const val QUEUE_LIMIT = 1024
        private const val READ_CHUNK = 16384
    }
}
