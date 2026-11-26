package com.remoteview.client.update

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.os.Build
import android.os.Handler
import android.os.Looper

/** Nhận kết quả phiên PackageInstaller; mở hộp xác nhận của hệ thống khi cần. */
class InstallResultReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                val confirm: Intent? = if (Build.VERSION.SDK_INT >= 33) {
                    intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
                } else {
                    @Suppress("DEPRECATION")
                    intent.getParcelableExtra(Intent.EXTRA_INTENT)
                }
                if (confirm != null) {
                    confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    try { context.startActivity(confirm) } catch (e: Exception) { post(false, e.message) }
                }
            }
            PackageInstaller.STATUS_SUCCESS -> post(true, null)
            else -> post(false, intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE) ?: "mã $status")
        }
    }

    private fun post(ok: Boolean, msg: String?) {
        Handler(Looper.getMainLooper()).post { listener?.invoke(ok, msg) }
    }

    companion object {
        const val ACTION = "com.remoteview.client.INSTALL_RESULT"
        @Volatile var listener: ((Boolean, String?) -> Unit)? = null
    }
}
