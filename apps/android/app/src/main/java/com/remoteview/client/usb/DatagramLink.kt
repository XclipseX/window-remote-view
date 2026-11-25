package com.remoteview.client.usb

import java.io.EOFException
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

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

    override fun send(bytes: ByteArray): Boolean {
        if (!isOpen || bytes.size > MAX_RECORD) return false
        return queue.offer(bytes)
    }

    /** Đóng tài nguyên bên dưới (socket / ParcelFileDescriptor). Phải an toàn khi gọi nhiều lần. */
    protected abstract fun closeResources()

    /** Chạy ở thread nền khi stream đã sẵn sàng. */
    protected fun runStreams(input: InputStream, output: OutputStream) {
        opened = true
        listener?.onOpen()
        writer = Thread({ writeLoop(output) }, "rv-$name-w").also { it.isDaemon = true; it.start() }
        // Thread đọc chính là thread hiện tại nếu gọi từ thread nền; tách riêng cho đơn giản.
        reader = Thread({ readLoop(input) }, "rv-$name-r").also { it.isDaemon = true; it.start() }
    }

    private fun readLoop(input: InputStream) {
        val header = ByteArray(4)
        var reason: String? = null
        try {
            while (!closed.get()) {
                readFully(input, header, 4)
                val len = ((header[0].toInt() and 0xFF) shl 24) or
                    ((header[1].toInt() and 0xFF) shl 16) or
                    ((header[2].toInt() and 0xFF) shl 8) or
                    (header[3].toInt() and 0xFF)
                if (len < 0 || len > MAX_RECORD) throw IOException("record dài bất thường: $len")
                val payload = ByteArray(len)
                if (len > 0) readFully(input, payload, len)
                listener?.onDatagram(payload)
            }
        } catch (e: Exception) {
            if (!closed.get()) reason = e.message ?: e.javaClass.simpleName
        }
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
            }
        } catch (e: Exception) {
            if (!closed.get()) reason = e.message ?: e.javaClass.simpleName
        }
        fail(reason)
    }

    private fun readFully(input: InputStream, buf: ByteArray, n: Int) {
        var off = 0
        while (off < n) {
            val r = input.read(buf, off, n - off)
            if (r < 0) throw EOFException("đầu kia đã đóng")
            off += r
        }
    }

    /** Đóng liên kết và báo một lần duy nhất. */
    protected fun fail(reason: String?) {
        if (!closed.compareAndSet(false, true)) return
        queue.clear()
        try { closeResources() } catch (_: Exception) {}
        listener?.onClosed(reason)
    }

    override fun close() = fail(null)

    companion object {
        const val MAX_RECORD = 64 * 1024
        private const val QUEUE_LIMIT = 1024
    }
}
