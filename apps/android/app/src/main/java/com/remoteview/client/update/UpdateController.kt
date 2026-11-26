package com.remoteview.client.update

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.remoteview.client.BuildConfig
import com.remoteview.client.RvLog
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import kotlin.coroutines.coroutineContext

sealed interface UpdateState {
    data object Idle : UpdateState
    data object Checking : UpdateState
    data object UpToDate : UpdateState
    data class Available(val info: UpdateInfo) : UpdateState
    /** [progress] 0..1, âm = chưa biết kích thước. */
    data class Downloading(val info: UpdateInfo, val progress: Float) : UpdateState
    data class NeedsPermission(val info: UpdateInfo) : UpdateState
    data class Installing(val info: UpdateInfo) : UpdateState
    /** API trả 404 (repo riêng tư / chưa có release): chỉ mở được trang phát hành bằng trình duyệt. */
    data object NotFound : UpdateState
    data class Failed(val message: String, val info: UpdateInfo?) : UpdateState
}

/** Kiểm tra / tải / cài bản cập nhật. Trạng thái là Compose state; mạng chạy ở Dispatchers.IO. */
class UpdateController(context: Context, private val scope: CoroutineScope) {
    private val app = context.applicationContext
    private val prefs = app.getSharedPreferences("update_state", Context.MODE_PRIVATE)
    private var job: Job? = null

    var state: UpdateState by mutableStateOf(UpdateState.Idle)
        private set

    /** Hộp thoại "Có bản cập nhật" (bật sau khi kiểm tra thấy bản mới, tắt khi người dùng đóng). */
    var dialogVisible by mutableStateOf(false)

    val currentSha: String get() = BuildConfig.GIT_SHA

    init {
        InstallResultReceiver.listener = { ok, msg ->
            state = if (ok) UpdateState.Idle else UpdateState.Failed(msg ?: "cài đặt thất bại", null)
        }
    }

    fun release() {
        InstallResultReceiver.listener = null
        job?.cancel()
    }

    /** Tự kiểm tra khi mở app: tối đa một lần mỗi 6 giờ. */
    fun autoCheck() {
        val last = prefs.getLong("last_check", 0L)
        if (System.currentTimeMillis() - last < MIN_INTERVAL_MS) return
        check(manual = false)
    }

    fun check(manual: Boolean) {
        if (busy()) return
        if (manual) state = UpdateState.Checking
        job = scope.launch {
            val r = withContext(Dispatchers.IO) { UpdateChecker.fetch() }
            when (r) {
                is FetchResult.Ok -> {
                    prefs.edit().putLong("last_check", System.currentTimeMillis()).apply()
                    if (UpdateChecker.sameCommit(r.info.remoteSha, currentSha)) {
                        state = UpdateState.UpToDate
                    } else {
                        state = UpdateState.Available(r.info)
                        dialogVisible = true
                    }
                }
                FetchResult.NotFound -> {
                    prefs.edit().putLong("last_check", System.currentTimeMillis()).apply()
                    state = if (manual) UpdateState.NotFound else UpdateState.Idle
                }
                is FetchResult.Error -> {
                    RvLog.w("kiểm tra cập nhật lỗi: ${r.message}")
                    state = if (manual) UpdateState.Failed(r.message, null) else UpdateState.Idle
                }
            }
        }
    }

    private fun busy() = state is UpdateState.Checking || state is UpdateState.Downloading || state is UpdateState.Installing

    fun dismissDialog() { dialogVisible = false }

    fun cancel() {
        job?.cancel()
        state = UpdateState.Idle
    }

    fun openReleasePage(url: String = UpdateChecker.RELEASE_PAGE) {
        try {
            app.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        } catch (e: Exception) {
            RvLog.w("không mở được trình duyệt: ${e.message}")
        }
    }

    /** Tải RemoteView.apk rồi cài. Không có asset thì mở trang phát hành. */
    fun startUpdate(info: UpdateInfo) {
        if (busy()) return
        dialogVisible = false
        val url = info.apkUrl
        if (url == null) {
            state = UpdateState.Failed("Bản phát hành không có tệp ${UpdateChecker.APK_NAME}", info)
            return
        }
        job = scope.launch {
            state = UpdateState.Downloading(info, if (info.apkSize > 0) 0f else -1f)
            try {
                withContext(Dispatchers.IO) { download(url, info) }
                install(info)
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                RvLog.w("tải cập nhật lỗi: ${e.message}")
                state = UpdateState.Failed(e.message ?: e.javaClass.simpleName, info)
            }
        }
    }

    private fun apkFile(): File = File(File(app.cacheDir, "update").also { it.mkdirs() }, UpdateChecker.APK_NAME)

    private suspend fun download(url: String, info: UpdateInfo) {
        val out = apkFile()
        val tmp = File(out.parentFile, UpdateChecker.APK_NAME + ".part")
        val conn = (URL(url).openConnection() as HttpURLConnection).apply {
            connectTimeout = 15_000
            readTimeout = 30_000
            instanceFollowRedirects = true
            setRequestProperty("User-Agent", "RemoteView-Android")
        }
        try {
            if (conn.responseCode != 200) throw java.io.IOException("HTTP ${conn.responseCode}")
            val total = conn.contentLengthLong.takeIf { it > 0 } ?: info.apkSize
            var done = 0L
            var lastPct = -1
            conn.inputStream.use { ins ->
                tmp.outputStream().use { os ->
                    val buf = ByteArray(64 * 1024)
                    while (true) {
                        coroutineContext.ensureActive()
                        val n = ins.read(buf)
                        if (n < 0) break
                        os.write(buf, 0, n)
                        done += n
                        if (total > 0) {
                            val pct = (done * 100 / total).toInt()
                            if (pct != lastPct) {
                                lastPct = pct
                                state = UpdateState.Downloading(info, pct / 100f)
                            }
                        }
                    }
                }
            }
            if (total > 0 && done != total) throw java.io.IOException("tải thiếu dữ liệu ($done/$total byte)")
            if (out.exists()) out.delete()
            if (!tmp.renameTo(out)) throw java.io.IOException("không ghi được tệp APK")
        } finally {
            conn.disconnect()
            if (tmp.exists()) tmp.delete()
        }
    }

    /** Gọi lại khi app trở lại foreground: nếu vừa cấp quyền "cài ứng dụng không rõ nguồn" thì cài tiếp. */
    fun onResume() {
        val s = state
        if (s is UpdateState.NeedsPermission && canInstall() && apkFile().exists()) {
            scope.launch { install(s.info) }
        }
    }

    private fun canInstall(): Boolean =
        Build.VERSION.SDK_INT < 26 || app.packageManager.canRequestPackageInstalls()

    private suspend fun install(info: UpdateInfo) {
        if (!canInstall()) {
            state = UpdateState.NeedsPermission(info)
            openUnknownSourcesSettings()
            return
        }
        state = UpdateState.Installing(info)
        try {
            withContext(Dispatchers.IO) { commitSession() }
        } catch (e: Exception) {
            RvLog.w("cài cập nhật lỗi: ${e.message}")
            state = UpdateState.Failed(e.message ?: e.javaClass.simpleName, info)
        }
    }

    fun openUnknownSourcesSettings() {
        try {
            app.startActivity(
                Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${app.packageName}"))
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        } catch (e: Exception) {
            RvLog.w("không mở được cài đặt nguồn không rõ: ${e.message}")
        }
    }

    private fun commitSession() {
        val file = apkFile()
        val installer = app.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
        params.setAppPackageName(app.packageName)
        val id = installer.createSession(params)
        installer.openSession(id).use { session ->
            file.inputStream().use { ins ->
                session.openWrite(UpdateChecker.APK_NAME, 0, file.length()).use { os ->
                    ins.copyTo(os)
                    session.fsync(os)
                }
            }
            val intent = Intent(app, InstallResultReceiver::class.java).setAction(InstallResultReceiver.ACTION)
            val flags = PendingIntent.FLAG_UPDATE_CURRENT or
                (if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0)
            val pi = PendingIntent.getBroadcast(app, id, intent, flags)
            session.commit(pi.intentSender)
        }
    }

    companion object {
        const val MIN_INTERVAL_MS = 6L * 60 * 60 * 1000
    }
}
