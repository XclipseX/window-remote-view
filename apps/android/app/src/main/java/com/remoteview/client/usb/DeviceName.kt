package com.remoteview.client.usb

import android.content.Context
import android.os.Build
import android.provider.Settings

/** Tên thân thiện của thiết bị này, gửi cho host trong hello JSON ("device_name"). */
object DeviceName {
    fun friendly(context: Context): String {
        val model = (Build.MODEL ?: "").trim()
        val up = model.uppercase()
        val known = when {
            up.startsWith("SM-X82") -> "Galaxy Tab S10+"
            up.startsWith("SM-S947") -> "Galaxy S26+"
            else -> null
        }
        if (known != null) return "$known ($model)"
        val user = try {
            Settings.Global.getString(context.contentResolver, "device_name")?.trim()
        } catch (_: Exception) {
            null
        }
        return when {
            user.isNullOrEmpty() -> model.ifEmpty { "Android" }
            user.equals(model, ignoreCase = true) || model.isEmpty() -> user
            else -> "$user ($model)"
        }
    }
}
