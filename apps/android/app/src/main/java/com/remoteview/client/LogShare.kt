package com.remoteview.client

import android.content.Context
import android.content.Intent
import android.os.Build
import androidx.core.content.FileProvider
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream

/** Chia sẻ file log (zip) qua ACTION_SEND và gửi log lên máy tính qua phiên đang mở. */
object LogShare {

    /** Nén toàn bộ log hiện tại + trước đó vào cache/share/remoteview-logs.zip. */
    fun zipAll(ctx: Context): File {
        val dir = File(ctx.cacheDir, "share").also { it.mkdirs() }
        val out = File(dir, "remoteview-logs.zip")
        ZipOutputStream(out.outputStream().buffered()).use { z ->
            for (f in RvLog.logFiles()) {
                z.putNextEntry(ZipEntry(f.name))
                f.inputStream().use { it.copyTo(z) }
                z.closeEntry()
            }
        }
        return out
    }

    fun share(ctx: Context) {
        val zip = zipAll(ctx)
        val uri = FileProvider.getUriForFile(ctx, "${ctx.packageName}.logs", zip)
        val send = Intent(Intent.ACTION_SEND).apply {
            type = "application/zip"
            putExtra(Intent.EXTRA_STREAM, uri)
            putExtra(Intent.EXTRA_SUBJECT, "Remote View log")
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        ctx.startActivity(Intent.createChooser(send, null).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }

    /** Tên đề xuất, ví dụ "sm-x820-20261126-101500". */
    fun uploadName(): String {
        val model = Build.MODEL.lowercase(Locale.US).replace(Regex("[^a-z0-9]+"), "-").trim('-').ifEmpty { "android" }
        return model + "-" + SimpleDateFormat("yyyyMMdd-HHmmss", Locale.US).format(Date())
    }

    /** Gửi log hiện tại lên máy tính. Chặn thread gọi: gọi từ thread nền. */
    fun upload(handle: Long): Boolean {
        if (!NativeBridge.available || handle == 0L) return false
        return try {
            val ok = NativeBridge.nativeUploadLog(handle, uploadName(), RvLog.currentFileBytes())
            RvLog.i("nativeUploadLog -> $ok")
            ok
        } catch (t: Throwable) {
            RvLog.w("nativeUploadLog lỗi", t)
            false
        }
    }
}
