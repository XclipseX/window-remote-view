package com.remoteview.client.update

import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL

/** Thông tin bản build mới nhất trên GitHub Releases (tag `latest-build`). */
data class UpdateInfo(
    val remoteSha: String,
    val apkUrl: String?,
    val apkSize: Long,
    val pageUrl: String,
)

sealed interface FetchResult {
    data class Ok(val info: UpdateInfo) : FetchResult
    /** 404: release không tồn tại hoặc repo riêng tư (API không đọc được khi chưa đăng nhập). */
    data object NotFound : FetchResult
    data class Error(val message: String) : FetchResult
}

object UpdateChecker {
    const val RELEASE_API = "https://api.github.com/repos/XclipseX/window-remote-view/releases/tags/latest-build"
    const val RELEASE_PAGE = "https://github.com/XclipseX/window-remote-view/releases/tag/latest-build"
    const val APK_NAME = "RemoteView.apk"

    private val SHA_RE = Regex("Bản build tự động từ commit\\s+([0-9a-fA-F]{7,40})")

    /** Gọi ở luồng nền (IO). */
    fun fetch(): FetchResult {
        var conn: HttpURLConnection? = null
        return try {
            conn = (URL(RELEASE_API).openConnection() as HttpURLConnection).apply {
                connectTimeout = 10_000
                readTimeout = 15_000
                setRequestProperty("Accept", "application/vnd.github+json")
                setRequestProperty("User-Agent", "RemoteView-Android")
            }
            when (val code = conn.responseCode) {
                200 -> {
                    val text = conn.inputStream.bufferedReader().use { it.readText() }
                    parse(text)
                }
                404 -> FetchResult.NotFound
                else -> FetchResult.Error("HTTP $code")
            }
        } catch (e: Exception) {
            FetchResult.Error(e.message ?: e.javaClass.simpleName)
        } finally {
            conn?.disconnect()
        }
    }

    internal fun parse(json: String): FetchResult {
        val o = JSONObject(json)
        val body = o.optString("body", "")
        val sha = SHA_RE.find(body)?.groupValues?.get(1)
            ?: return FetchResult.Error("Không đọc được mã commit trong nội dung bản phát hành")
        var apk: String? = null
        var size = 0L
        val assets = o.optJSONArray("assets")
        if (assets != null) {
            for (i in 0 until assets.length()) {
                val a = assets.getJSONObject(i)
                if (a.optString("name") == APK_NAME) {
                    apk = a.optString("browser_download_url").ifBlank { null }
                    size = a.optLong("size", 0L)
                    break
                }
            }
        }
        val page = o.optString("html_url", RELEASE_PAGE).ifBlank { RELEASE_PAGE }
        return FetchResult.Ok(UpdateInfo(sha, apk, size, page))
    }

    /** true nếu hai mã commit là một (cho phép so sánh dạng rút gọn). */
    fun sameCommit(a: String, b: String): Boolean {
        if (a.isBlank() || b.isBlank() || a == "dev" || b == "dev") return false
        val x = a.lowercase()
        val y = b.lowercase()
        return x.startsWith(y) || y.startsWith(x)
    }
}
