package com.remoteview.client.ui

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** Lưới 8dp dùng thống nhất cho khoảng cách. */
object Spacing {
    val xs = 4.dp
    val s = 8.dp
    val m = 16.dp
    val l = 24.dp
    val xl = 32.dp
}

/** Màu ngữ nghĩa không có trong Material 3 (thành công / cảnh báo). */
@Immutable
class StatusColors(
    val success: Color,
    val onSuccess: Color,
    val successContainer: Color,
    val onSuccessContainer: Color,
    val warningContainer: Color,
    val onWarningContainer: Color,
)

val LocalStatusColors = staticCompositionLocalOf {
    StatusColors(Color(0xFF1B6B3A), Color.White, Color(0xFFA8F2BC), Color(0xFF00210E), Color(0xFFFFDEA6), Color(0xFF261900))
}

// Bảng màu dự phòng (Android < 12): xanh dương-lục, nền trung tính.
private val FallbackLight: ColorScheme = lightColorScheme(
    primary = Color(0xFF00629D),
    onPrimary = Color.White,
    primaryContainer = Color(0xFFCFE5FF),
    onPrimaryContainer = Color(0xFF001D33),
    secondary = Color(0xFF526070),
    secondaryContainer = Color(0xFFD5E4F7),
    onSecondaryContainer = Color(0xFF0F1D2A),
    tertiary = Color(0xFF00696A),
    tertiaryContainer = Color(0xFF9CF1F1),
    onTertiaryContainer = Color(0xFF002020),
    background = Color(0xFFF8F9FF),
    onBackground = Color(0xFF191C20),
    surface = Color(0xFFF8F9FF),
    onSurface = Color(0xFF191C20),
    surfaceVariant = Color(0xFFDEE3EB),
    onSurfaceVariant = Color(0xFF42474E),
    surfaceContainerLowest = Color.White,
    surfaceContainerLow = Color(0xFFF2F3FA),
    surfaceContainer = Color(0xFFECEEF4),
    surfaceContainerHigh = Color(0xFFE6E8EE),
    surfaceContainerHighest = Color(0xFFE0E2E8),
    outline = Color(0xFF72787F),
    outlineVariant = Color(0xFFC2C7CF),
)

private val FallbackDark: ColorScheme = darkColorScheme(
    primary = Color(0xFF9ACBFF),
    onPrimary = Color(0xFF003354),
    primaryContainer = Color(0xFF004A77),
    onPrimaryContainer = Color(0xFFCFE5FF),
    secondary = Color(0xFFB9C8DA),
    secondaryContainer = Color(0xFF3A4857),
    onSecondaryContainer = Color(0xFFD5E4F7),
    tertiary = Color(0xFF80D5D5),
    tertiaryContainer = Color(0xFF004F50),
    onTertiaryContainer = Color(0xFF9CF1F1),
    background = Color(0xFF101418),
    onBackground = Color(0xFFE0E2E8),
    surface = Color(0xFF101418),
    onSurface = Color(0xFFE0E2E8),
    surfaceVariant = Color(0xFF42474E),
    onSurfaceVariant = Color(0xFFC2C7CF),
    surfaceContainerLowest = Color(0xFF0B0F13),
    surfaceContainerLow = Color(0xFF181C20),
    surfaceContainer = Color(0xFF1D2024),
    surfaceContainerHigh = Color(0xFF272A2F),
    surfaceContainerHighest = Color(0xFF32353A),
    outline = Color(0xFF8C9199),
    outlineVariant = Color(0xFF42474E),
)

private val LightStatus = StatusColors(
    success = Color(0xFF1B6B3A), onSuccess = Color.White,
    successContainer = Color(0xFFA8F2BC), onSuccessContainer = Color(0xFF00210E),
    warningContainer = Color(0xFFFFDEA6), onWarningContainer = Color(0xFF261900),
)
private val DarkStatus = StatusColors(
    success = Color(0xFF8DD5A1), onSuccess = Color(0xFF00391C),
    successContainer = Color(0xFF005229), onSuccessContainer = Color(0xFFA8F2BC),
    warningContainer = Color(0xFF5B4300), onWarningContainer = Color(0xFFFFDEA6),
)

private val AppTypography: Typography = Typography().let { t ->
    t.copy(
        headlineMedium = t.headlineMedium.copy(fontWeight = FontWeight.SemiBold),
        headlineSmall = t.headlineSmall.copy(fontWeight = FontWeight.SemiBold),
        titleLarge = t.titleLarge.copy(fontWeight = FontWeight.SemiBold),
        titleMedium = t.titleMedium.copy(fontWeight = FontWeight.SemiBold),
        bodyLarge = t.bodyLarge.copy(fontSize = 16.sp, lineHeight = 24.sp),
        bodyMedium = t.bodyMedium.copy(fontSize = 14.sp, lineHeight = 20.sp),
        labelLarge = TextStyle(fontWeight = FontWeight.Medium, fontSize = 14.sp, lineHeight = 20.sp),
    )
}

@Composable
fun RemoteViewTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val colors = when {
        Build.VERSION.SDK_INT >= 31 ->
            if (dark) dynamicDarkColorScheme(LocalContext.current) else dynamicLightColorScheme(LocalContext.current)
        dark -> FallbackDark
        else -> FallbackLight
    }
    CompositionLocalProvider(LocalStatusColors provides (if (dark) DarkStatus else LightStatus)) {
        MaterialTheme(colorScheme = colors, typography = AppTypography, content = content)
    }
}
