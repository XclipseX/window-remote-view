package com.remoteview.client.ui

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Home
import androidx.compose.material.icons.rounded.Settings
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationRail
import androidx.compose.material3.NavigationRailItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.windowsizeclass.ExperimentalMaterial3WindowSizeClassApi
import androidx.compose.material3.windowsizeclass.WindowWidthSizeClass
import androidx.compose.material3.windowsizeclass.calculateWindowSizeClass
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import com.remoteview.client.R

enum class WidthClass { COMPACT, MEDIUM, EXPANDED }

private fun Context.findActivity(): Activity? {
    var c: Context? = this
    while (c is ContextWrapper) {
        if (c is Activity) return c
        c = c.baseContext
    }
    return null
}

/** Lớp chiều rộng cửa sổ (WindowSizeClass): điện thoại = COMPACT, máy tính bảng = MEDIUM/EXPANDED. */
@OptIn(ExperimentalMaterial3WindowSizeClassApi::class)
@Composable
fun rememberWidthClass(): WidthClass {
    val activity = LocalContext.current.findActivity()
    if (activity != null) {
        return when (calculateWindowSizeClass(activity).widthSizeClass) {
            WindowWidthSizeClass.Compact -> WidthClass.COMPACT
            WindowWidthSizeClass.Medium -> WidthClass.MEDIUM
            else -> WidthClass.EXPANDED
        }
    }
    val w = LocalConfiguration.current.screenWidthDp
    return if (w < 600) WidthClass.COMPACT else if (w < 840) WidthClass.MEDIUM else WidthClass.EXPANDED
}

private class NavItem(val labelRes: Int, val icon: ImageVector)

private val NAV_ITEMS = listOf(
    NavItem(R.string.nav_home, Icons.Rounded.Home),
    NavItem(R.string.nav_settings, Icons.Rounded.Settings),
)

/** Khung điều hướng thích ứng: NavigationBar (compact) hoặc NavigationRail (medium/expanded). */
@Composable
fun AppShell(
    selected: Int,
    onSelect: (Int) -> Unit,
    widthClass: WidthClass,
    content: @Composable (PaddingValues) -> Unit,
) {
    if (widthClass == WidthClass.COMPACT) {
        Scaffold(
            containerColor = MaterialTheme.colorScheme.surface,
            bottomBar = {
                NavigationBar {
                    NAV_ITEMS.forEachIndexed { i, item ->
                        NavigationBarItem(
                            selected = selected == i,
                            onClick = { onSelect(i) },
                            icon = { Icon(item.icon, contentDescription = null) },
                            label = { Text(stringResource(item.labelRes)) },
                        )
                    }
                }
            },
        ) { padding -> content(padding) }
    } else {
        Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.surface) {
            Row(Modifier.fillMaxSize()) {
                NavigationRail(containerColor = MaterialTheme.colorScheme.surfaceContainer) {
                    NAV_ITEMS.forEachIndexed { i, item ->
                        NavigationRailItem(
                            selected = selected == i,
                            onClick = { onSelect(i) },
                            icon = { Icon(item.icon, contentDescription = null) },
                            label = { Text(stringResource(item.labelRes)) },
                        )
                    }
                }
                Box(
                    Modifier.weight(1f).fillMaxSize()
                        .windowInsetsPadding(
                            WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.End + WindowInsetsSides.Bottom),
                        ),
                ) { content(PaddingValues()) }
            }
        }
    }
}
