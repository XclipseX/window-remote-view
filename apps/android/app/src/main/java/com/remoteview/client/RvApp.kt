package com.remoteview.client

import android.app.Application

/** Mở file log ngay khi tiến trình khởi động để không mất phần đầu nhật ký; ghi cả crash chưa bắt. */
class RvApp : Application() {
    override fun onCreate() {
        super.onCreate()
        RvLog.init(this)
        RvLog.i("RvApp.onCreate pid=${android.os.Process.myPid()}")
        val prev = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { t, e ->
            try {
                RvLog.e("CRASH thread=${t.name}\n" + android.util.Log.getStackTraceString(e))
                RvLog.flush()
            } catch (_: Throwable) {}
            prev?.uncaughtException(t, e)
        }
    }
}
