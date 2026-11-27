package com.remoteview.client.update

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class UpdateCheckerTest {

    private val validApkHex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    private val validExeHex = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"
    private val digestHex = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

    @Test
    fun parseAssetSha256MatchesReleaseBodyLine() {
        val body = """
            Bản build tự động từ commit 1234567890abcdef
            - SHA256(RemoteView.exe): $validExeHex
            - SHA256(RemoteView.apk): $validApkHex
            - RemoteView.apk: cài lên Android
        """.trimIndent()

        assertEquals(
            validApkHex,
            UpdateChecker.parseAssetSha256(body, UpdateChecker.APK_NAME),
        )
        assertEquals(
            validExeHex,
            UpdateChecker.parseAssetSha256(body, "RemoteView.exe"),
        )
    }

    @Test
    fun parseAssetSha256PrefersBodyOverApiDigest() {
        val body = "SHA256(RemoteView.apk): $validApkHex"
        val got = UpdateChecker.parseAssetSha256(
            body = body,
            assetName = UpdateChecker.APK_NAME,
            assetDigest = "sha256:$digestHex",
        )
        assertEquals(validApkHex, got)
    }

    @Test
    fun parseAssetSha256FallsBackToGitHubAssetDigest() {
        val body = "Bản build tự động từ commit abcdef123456"
        val got = UpdateChecker.parseAssetSha256(
            body = body,
            assetName = UpdateChecker.APK_NAME,
            assetDigest = "sha256:$digestHex",
        )
        assertEquals(digestHex, got)
    }

    @Test
    fun parseAssetSha256IsCaseInsensitiveAndNormalizesLowercase() {
        val upper = validApkHex.uppercase()
        val body = "sha256(remoteview.apk):   $upper"
        assertEquals(
            validApkHex,
            UpdateChecker.parseAssetSha256(body, UpdateChecker.APK_NAME),
        )

        val upperDigest = "SHA256:${digestHex.uppercase()}"
        assertEquals(
            digestHex,
            UpdateChecker.parseAssetSha256("", UpdateChecker.APK_NAME, upperDigest),
        )
    }

    @Test
    fun parseAssetSha256FallsBackWhenBodyHashIsInvalid() {
        val body = "SHA256(RemoteView.apk): not-a-valid-sha256"
        val got = UpdateChecker.parseAssetSha256(
            body = body,
            assetName = UpdateChecker.APK_NAME,
            assetDigest = "sha256:$digestHex",
        )
        assertEquals(digestHex, got)
    }

    @Test
    fun parseAssetSha256ReturnsNullForMissingOrInvalidHex() {
        assertNull(UpdateChecker.parseAssetSha256("", UpdateChecker.APK_NAME, null))
        assertNull(UpdateChecker.parseAssetSha256("no checksum here", UpdateChecker.APK_NAME, ""))

        // 63 hex chars (too short)
        val tooShort = validApkHex.dropLast(1)
        assertNull(UpdateChecker.parseAssetSha256("SHA256(RemoteView.apk): $tooShort", UpdateChecker.APK_NAME))
        assertNull(UpdateChecker.parseAssetSha256("", UpdateChecker.APK_NAME, "sha256:$tooShort"))

        // 65 hex chars (too long)
        val tooLong = validApkHex + "a"
        assertNull(UpdateChecker.parseAssetSha256("SHA256(RemoteView.apk): $tooLong", UpdateChecker.APK_NAME))
        assertNull(UpdateChecker.parseAssetSha256("", UpdateChecker.APK_NAME, "sha256:$tooLong"))

        // Non-hex character
        val nonHex = "g" + validApkHex.drop(1)
        assertNull(UpdateChecker.parseAssetSha256("SHA256(RemoteView.apk): $nonHex", UpdateChecker.APK_NAME))
        assertNull(UpdateChecker.parseAssetSha256("", UpdateChecker.APK_NAME, "sha256:$nonHex"))

        // Wrong digest algorithm prefix
        assertNull(UpdateChecker.parseAssetSha256("", UpdateChecker.APK_NAME, "md5:$validApkHex"))
        assertNull(UpdateChecker.parseAssetSha256("", UpdateChecker.APK_NAME, validApkHex))

        // Different asset name (ensure regex escapes dots in assetName)
        assertNull(
            UpdateChecker.parseAssetSha256(
                "SHA256(RemoteViewXapk): $validApkHex",
                UpdateChecker.APK_NAME,
            ),
        )
    }

    @Test
    fun sameCommitHandlesFullAndShortShas() {
        assertTrue(UpdateChecker.sameCommit("abcdef1234567890", "ABCDEF1"))
        assertTrue(UpdateChecker.sameCommit("abcdef1", "abcdef1234567890"))
        assertFalse(UpdateChecker.sameCommit("dev", "abcdef1"))
        assertFalse(UpdateChecker.sameCommit("", "abcdef1"))
        assertFalse(UpdateChecker.sameCommit("abcdef1", "1234567"))
    }
}
