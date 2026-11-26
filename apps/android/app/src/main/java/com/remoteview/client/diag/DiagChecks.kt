package com.remoteview.client.diag

import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.hardware.display.DisplayManager
import android.hardware.usb.UsbManager
import android.media.AudioManager
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.wifi.WifiManager
import android.os.BatteryManager
import android.os.Build
import android.os.PowerManager
import android.os.SystemClock
import android.provider.Settings
import android.telephony.TelephonyManager
import android.view.Display
import androidx.annotation.StringRes
import com.remoteview.client.R
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.Inet4Address
import java.net.Inet6Address
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.NetworkInterface
import java.net.Socket
import java.net.SocketTimeoutException
import java.util.Locale

// UNBUILT: chưa biên dịch.

/** Các mục chẩn đoán. Mọi hàm chạy blocking: gọi từ thread nền (Dispatchers.IO). */
class DiagChecks(context: Context, private val facts: DiagFacts) {
    private val ctx: Context = context.applicationContext
    private val pm: PackageManager = ctx.packageManager

    private fun s(@StringRes id: Int, vararg a: Any): String = ctx.getString(id, *a)
    private fun yn(b: Boolean?): String = when (b) {
        true -> s(R.string.diag_yes)
        false -> s(R.string.diag_no)
        null -> s(R.string.diag_unknown)
    }
    private fun kv(@StringRes label: Int, value: String): String = s(R.string.diag_kv, s(label), value)

    // ---------------------------------------------------------------- 1. Thiết bị
    fun device(): CheckResult {
        val model = Build.MODEL ?: ""
        val up = model.uppercase(Locale.ROOT)
        val known = when {
            up.startsWith("SM-X820") -> s(R.string.diag_dev_x820)
            up.startsWith("SM-X826") -> s(R.string.diag_dev_x826)
            up.startsWith("SM-X920") -> s(R.string.diag_dev_x920)
            up.startsWith("SM-X926") -> s(R.string.diag_dev_x926)
            else -> null
        }
        val details = mutableListOf<String>()
        details += kv(R.string.diag_l_manufacturer, "${Build.MANUFACTURER} (${Build.BRAND})")
        details += kv(R.string.diag_l_model, "$model (${Build.DEVICE})")
        details += kv(R.string.diag_l_android, "${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT})")
        details += kv(R.string.diag_l_patch, Build.VERSION.SECURITY_PATCH ?: "?")
        details += kv(R.string.diag_l_build, Build.DISPLAY ?: "?")
        if (Build.VERSION.SDK_INT >= 31) {
            details += kv(R.string.diag_l_soc, "${Build.SOC_MANUFACTURER} ${Build.SOC_MODEL}")
        } else {
            details += kv(R.string.diag_l_soc, s(R.string.diag_unavailable_api))
        }
        details += kv(R.string.diag_l_oneui, s(R.string.diag_oneui_skipped))
        if (known != null) details += known
        return CheckResult(
            id = "device",
            title = s(R.string.diag_c_device),
            verdict = Verdict.INFO,
            headline = "${Build.MANUFACTURER} $model - Android ${Build.VERSION.RELEASE}",
            details = details,
            explanation = s(R.string.diag_device_explain),
        )
    }

    // ---------------------------------------------------------------- 2. Mạng di động
    fun cellular(): CheckResult {
        val hasTel = pm.hasSystemFeature(PackageManager.FEATURE_TELEPHONY)
        val tm = ctx.getSystemService(Context.TELEPHONY_SERVICE) as? TelephonyManager
        val phoneType = try { tm?.phoneType } catch (_: Exception) { null }
        val simState = try { tm?.simState } catch (_: Exception) { null }
        val noPhone = phoneType == null || phoneType == TelephonyManager.PHONE_TYPE_NONE
        val wifiOnly = !hasTel || noPhone
        facts.wifiOnly = wifiOnly
        val details = listOf(
            kv(R.string.diag_l_feature_tel, yn(hasTel)),
            kv(R.string.diag_l_phone_type, phoneTypeName(phoneType)),
            kv(R.string.diag_l_sim_state, simState?.toString() ?: s(R.string.diag_unknown)),
        )
        return if (wifiOnly) {
            CheckResult(
                "cellular", s(R.string.diag_c_cellular), Verdict.WARN,
                s(R.string.diag_cell_wifi_only), details,
                s(R.string.diag_cell_wifi_only_explain), s(R.string.diag_cell_wifi_only_fix),
            )
        } else {
            CheckResult(
                "cellular", s(R.string.diag_c_cellular), Verdict.OK,
                s(R.string.diag_cell_has), details,
                s(R.string.diag_cell_has_explain), s(R.string.diag_cell_has_fix),
            )
        }
    }

    private fun phoneTypeName(t: Int?): String = when (t) {
        null -> s(R.string.diag_unknown)
        TelephonyManager.PHONE_TYPE_NONE -> "NONE"
        TelephonyManager.PHONE_TYPE_GSM -> "GSM"
        TelephonyManager.PHONE_TYPE_CDMA -> "CDMA"
        TelephonyManager.PHONE_TYPE_SIP -> "SIP"
        else -> t.toString()
    }

    // ---------------------------------------------------------------- 3. Trạng thái USB
    fun usbState(): CheckResult {
        val snap = UsbStates.readSticky(ctx)
        facts.usb = snap
        val details = mutableListOf<String>()
        details += kv(R.string.diag_l_usb_connected, yn(snap.connected))
        details += kv(R.string.diag_l_usb_configured, yn(snap.configured))
        for (name in listOf("mtp", "ptp", "rndis", "ncm", "adb", "midi", "accessory")) {
            details += "$name: ${yn(snap.fn(name))}"
        }
        val others = snap.extras.keys.filter {
            it !in setOf("connected", "configured", "mtp", "ptp", "rndis", "ncm", "adb", "midi", "accessory")
        }
        if (others.isNotEmpty()) details += kv(R.string.diag_l_usb_other, others.joinToString { "$it=${snap.extras[it]}" })
        val active = snap.activeFunctions()
        details += kv(R.string.diag_l_usb_function, if (active.isEmpty()) s(R.string.diag_none) else active.joinToString())

        val title = s(R.string.diag_c_usb)
        return when {
            !snap.broadcastSeen -> CheckResult(
                "usb", title, Verdict.INFO, s(R.string.diag_usb_nobroadcast), details,
                s(R.string.diag_usb_nobroadcast_explain), s(R.string.diag_usb_plug_fix),
            )
            snap.connected == false -> CheckResult(
                "usb", title, Verdict.WARN, s(R.string.diag_usb_none), details,
                s(R.string.diag_usb_none_explain), s(R.string.diag_usb_plug_fix),
            )
            snap.connected == true && snap.configured == false && !snap.dataFunctionActive -> CheckResult(
                "usb", title, Verdict.FAIL, s(R.string.diag_usb_chargeonly), details,
                s(R.string.diag_usb_chargeonly_explain), s(R.string.diag_usb_chargeonly_fix),
            )
            snap.connected == true && (snap.configured == true || snap.dataFunctionActive) -> {
                val fnTxt = if (active.isEmpty()) "?" else active.joinToString()
                CheckResult(
                    "usb", title, Verdict.OK, s(R.string.diag_usb_data, fnTxt), details,
                    s(R.string.diag_usb_data_explain), s(R.string.diag_usb_data_fix),
                )
            }
            else -> CheckResult(
                "usb", title, Verdict.INFO, s(R.string.diag_usb_unknown), details,
                s(R.string.diag_usb_unknown_explain), s(R.string.diag_usb_plug_fix),
            )
        }
    }

    // ---------------------------------------------------------------- 4. USB accessory (AOA)
    fun accessory(): CheckResult {
        val hasAcc = pm.hasSystemFeature(PackageManager.FEATURE_USB_ACCESSORY)
        val hasHost = pm.hasSystemFeature(PackageManager.FEATURE_USB_HOST)
        val details = mutableListOf<String>()
        details += kv(R.string.diag_l_feature_acc, yn(hasAcc))
        details += kv(R.string.diag_l_feature_host, yn(hasHost))
        var ours = false
        var granted: Boolean? = null
        var count = 0
        try {
            val um = ctx.getSystemService(Context.USB_SERVICE) as? UsbManager
            val list = um?.accessoryList
            count = list?.size ?: 0
            list?.forEach { a ->
                val isOurs = a.manufacturer == "RemoteView"
                val perm = try { um.hasPermission(a) } catch (_: Exception) { null }
                details += s(R.string.diag_acc_item, a.manufacturer ?: "?", a.model ?: "?", yn(perm))
                if (isOurs) { ours = true; granted = perm }
            }
        } catch (e: Exception) {
            details += s(R.string.diag_error_fmt, e.javaClass.simpleName)
        }
        details += kv(R.string.diag_l_acc_count, count.toString())
        facts.accessoryAttached = ours
        val title = s(R.string.diag_c_aoa)
        return when {
            !hasAcc -> CheckResult("aoa", title, Verdict.FAIL, s(R.string.diag_aoa_unsupported), details,
                s(R.string.diag_aoa_unsupported_explain), s(R.string.diag_aoa_unsupported_fix))
            ours && granted == true -> CheckResult("aoa", title, Verdict.OK, s(R.string.diag_aoa_ok), details,
                s(R.string.diag_aoa_ok_explain), null)
            ours -> CheckResult("aoa", title, Verdict.WARN, s(R.string.diag_aoa_noperm), details,
                s(R.string.diag_aoa_noperm_explain), s(R.string.diag_aoa_noperm_fix))
            else -> CheckResult("aoa", title, Verdict.WARN, s(R.string.diag_aoa_waiting), details,
                s(R.string.diag_aoa_waiting_explain), s(R.string.diag_aoa_waiting_fix))
        }
    }

    // ---------------------------------------------------------------- 5. Tùy chọn nhà phát triển / ADB
    fun developer(): CheckResult {
        fun read(key: String): Boolean? = try {
            Settings.Global.getInt(ctx.contentResolver, key, 0) == 1
        } catch (_: Exception) { null }
        val dev = read(Settings.Global.DEVELOPMENT_SETTINGS_ENABLED)
        val adb = read(Settings.Global.ADB_ENABLED)
        facts.adbEnabled = adb
        val details = listOf(
            kv(R.string.diag_l_dev_opts, yn(dev)),
            kv(R.string.diag_l_adb, yn(adb)),
        )
        val title = s(R.string.diag_c_dev)
        return if (adb == true) {
            CheckResult("dev", title, Verdict.OK, s(R.string.diag_dev_adb_on), details, s(R.string.diag_dev_explain), null)
        } else {
            CheckResult("dev", title, Verdict.INFO, s(R.string.diag_dev_adb_off), details,
                s(R.string.diag_dev_explain), s(R.string.diag_dev_fix))
        }
    }

    // ---------------------------------------------------------------- 6. Giao diện mạng
    @Suppress("DEPRECATION")
    fun network(): CheckResult {
        val details = mutableListOf<String>()
        var tether = false
        try {
            val all = NetworkInterface.getNetworkInterfaces()?.toList().orEmpty().sortedBy { it.name }
            for (ni in all) {
                val up = try { ni.isUp } catch (_: Exception) { false }
                val lo = try { ni.isLoopback } catch (_: Exception) { false }
                val addrs = ni.inetAddresses.toList()
                val v4 = addrs.filterIsInstance<Inet4Address>().map { it.hostAddress ?: "?" }
                val v6 = addrs.filterIsInstance<Inet6Address>().map { (it.hostAddress ?: "?").substringBefore('%') }
                val usbLike = Regex("^(rndis|usb|ncm)\\d*$").matches(ni.name)
                if (usbLike && up && v4.isNotEmpty()) tether = true
                details += s(
                    R.string.diag_if_item,
                    ni.name,
                    if (up) "up" else "down",
                    if (lo) " loopback" else "",
                    if (usbLike) " [USB/tether]" else "",
                    if (v4.isEmpty()) "-" else v4.joinToString(),
                    if (v6.isEmpty()) "-" else v6.joinToString(),
                )
            }
        } catch (e: Exception) {
            details += s(R.string.diag_error_fmt, e.javaClass.simpleName)
        }
        facts.tetherActive = tether

        // Wi-Fi / kết nối hiện tại (không lấy SSID, không cần quyền vị trí).
        var wifiLine: String? = null
        try {
            val cm = ctx.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
            val active = cm.activeNetwork
            val caps = active?.let { cm.getNetworkCapabilities(it) }
            if (caps == null) {
                details += kv(R.string.diag_l_active_net, s(R.string.diag_none))
            } else {
                val tr = transportNames(caps)
                val validated = caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)
                val metered = !caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)
                details += kv(R.string.diag_l_active_net,
                    s(R.string.diag_net_active_fmt, tr, yn(validated), yn(metered)))
            }
            val allTr = cm.allNetworks.mapNotNull { cm.getNetworkCapabilities(it) }.joinToString(" | ") { transportNames(it) }
            details += kv(R.string.diag_l_all_net, if (allTr.isEmpty()) s(R.string.diag_none) else allTr)
            val wm = ctx.getSystemService(Context.WIFI_SERVICE) as? WifiManager
            if (wm != null) {
                details += kv(R.string.diag_l_wifi_enabled, yn(wm.isWifiEnabled))
                val info = wm.connectionInfo
                if (info != null && info.networkId != -1) {
                    wifiLine = s(R.string.diag_wifi_fmt, info.linkSpeed, info.rssi, info.frequency)
                    details += kv(R.string.diag_l_wifi_link, wifiLine)
                }
            }
        } catch (e: Exception) {
            details += s(R.string.diag_error_fmt, e.javaClass.simpleName)
        }
        val title = s(R.string.diag_c_net)
        return if (tether) {
            CheckResult("net", title, Verdict.OK, s(R.string.diag_net_tether), details, s(R.string.diag_net_tether_explain), null)
        } else {
            CheckResult("net", title, Verdict.INFO, s(R.string.diag_net_notether), details,
                s(R.string.diag_net_notether_explain), s(R.string.diag_net_notether_fix))
        }
    }

    private fun transportNames(c: NetworkCapabilities): String {
        val l = mutableListOf<String>()
        if (c.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)) l += "Wi-Fi"
        if (c.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR)) l += "Cellular"
        if (c.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)) l += "Ethernet"
        if (c.hasTransport(NetworkCapabilities.TRANSPORT_BLUETOOTH)) l += "Bluetooth"
        if (c.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) l += "VPN"
        if (Build.VERSION.SDK_INT >= 31 && c.hasTransport(NetworkCapabilities.TRANSPORT_USB)) l += "USB"
        return if (l.isEmpty()) "?" else l.joinToString("+")
    }

    // ---------------------------------------------------------------- 7. Thử kết nối cục bộ
    fun reachability(): CheckResult {
        val details = mutableListOf<String>()
        val adbOpen = try {
            Socket().use { it.connect(InetSocketAddress("127.0.0.1", 47810), 400) }
            true
        } catch (_: Exception) { false }
        facts.adbReverseOpen = adbOpen
        details += kv(R.string.diag_l_adb_reverse, yn(adbOpen))

        val hosts = LinkedHashMap<String, String>()
        var udpError: String? = null
        val wm = ctx.getSystemService(Context.WIFI_SERVICE) as? WifiManager
        val lock = try { wm?.createMulticastLock("rv-diag")?.apply { setReferenceCounted(false); acquire() } } catch (_: Exception) { null }
        try {
            val sock = try {
                DatagramSocket(null as java.net.SocketAddress?).apply { reuseAddress = true; bind(InetSocketAddress(47800)) }
            } catch (_: Exception) { DatagramSocket() }
            sock.use { sk ->
                sk.broadcast = true
                val payload = "RVDIAG-DISCOVER".toByteArray()
                val targets = linkedSetOf<InetAddress>()
                try { targets += InetAddress.getByName("255.255.255.255") } catch (_: Exception) {}
                try {
                    NetworkInterface.getNetworkInterfaces()?.toList().orEmpty().forEach { ni ->
                        if (ni.isUp && !ni.isLoopback) ni.interfaceAddresses.forEach { ia -> ia.broadcast?.let { targets += it } }
                    }
                } catch (_: Exception) {}
                for (t in targets) {
                    try { sk.send(DatagramPacket(payload, payload.size, t, 47800)) } catch (_: Exception) {}
                }
                val end = SystemClock.elapsedRealtime() + 1500
                val buf = ByteArray(1500)
                while (true) {
                    val left = end - SystemClock.elapsedRealtime()
                    if (left <= 0) break
                    sk.soTimeout = left.toInt().coerceIn(1, 300)
                    try {
                        val p = DatagramPacket(buf, buf.size)
                        sk.receive(p)
                        val data = p.data.copyOfRange(0, p.length)
                        if (data.contentEquals(payload)) continue // gói của chính mình (loopback)
                        val addr = p.address?.hostAddress ?: "?"
                        val text = String(data, Charsets.ISO_8859_1).take(48).map { if (it in ' '..'~') it else '.' }.joinToString("")
                        if (addr !in hosts) hosts[addr] = text
                    } catch (_: SocketTimeoutException) {
                    }
                }
            }
        } catch (e: Exception) {
            udpError = e.javaClass.simpleName
        } finally {
            try { lock?.release() } catch (_: Exception) {}
        }
        facts.hostsFound = hosts.size
        if (udpError != null) details += s(R.string.diag_error_fmt, udpError)
        details += kv(R.string.diag_l_hosts_found, hosts.size.toString())
        hosts.forEach { (a, t) -> details += s(R.string.diag_host_item, a, t) }

        val title = s(R.string.diag_c_reach)
        val verdict = if (adbOpen || hosts.isNotEmpty()) Verdict.OK else Verdict.WARN
        val headline = when {
            adbOpen && hosts.isNotEmpty() -> s(R.string.diag_reach_both)
            adbOpen -> s(R.string.diag_reach_adb)
            hosts.isNotEmpty() -> s(R.string.diag_reach_lan, hosts.size)
            else -> s(R.string.diag_reach_none)
        }
        return CheckResult("reach", title, verdict, headline, details, s(R.string.diag_reach_explain),
            if (verdict == Verdict.OK) null else s(R.string.diag_reach_fix))
    }

    // ---------------------------------------------------------------- 8. Ứng dụng liên quan
    @Suppress("DEPRECATION")
    fun relatedApps(): CheckResult {
        val pkgs = listOf("ph.spacedesk.beta", "ph.spacedesk.httpwww.spacedesk")
        val states = pkgs.map { p ->
            p to (try { pm.getPackageInfo(p, 0); true } catch (_: PackageManager.NameNotFoundException) { false } catch (_: Exception) { null })
        }
        val details = states.map { (p, inst) ->
            kv(R.string.diag_l_pkg, "$p = ${if (inst == null) s(R.string.diag_unknown) else if (inst) s(R.string.diag_installed) else s(R.string.diag_not_installed)}")
        }
        val any = states.any { it.second == true }
        facts.spacedeskInstalled = any
        return CheckResult("apps", s(R.string.diag_c_apps), Verdict.INFO,
            if (any) s(R.string.diag_apps_installed) else s(R.string.diag_apps_none), details,
            s(R.string.diag_apps_explain), null)
    }

    // ---------------------------------------------------------------- 9. Bộ giải mã video
    fun codecs(): CheckResult {
        val details = mutableListOf<String>()
        val mimes = listOf(
            MediaFormat.MIMETYPE_VIDEO_AVC to "avc",
            MediaFormat.MIMETYPE_VIDEO_HEVC to "hevc",
            MediaFormat.MIMETYPE_VIDEO_AV1 to "av1",
            MediaFormat.MIMETYPE_VIDEO_VP9 to "vp9",
        )
        // mime -> có bộ giải mã phần cứng chịu được 2800x1752@120
        val hwOk = HashMap<String, Boolean>()
        val swOk = HashMap<String, Boolean>()
        var total = 0
        try {
            val infos = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.filter { !it.isEncoder }
            for ((mime, label) in mimes) {
                val list = infos.filter { ci -> ci.supportedTypes.any { it.equals(mime, true) } }
                if (list.isEmpty()) { details += s(R.string.diag_codec_missing, label); continue }
                var vendorProbed = false
                for (ci in list) {
                    total++
                    val hw = if (Build.VERSION.SDK_INT >= 29) ci.isHardwareAccelerated else !ci.name.lowercase(Locale.ROOT).startsWith("omx.google") && !ci.name.lowercase(Locale.ROOT).startsWith("c2.android")
                    var ll: Boolean? = null
                    var okSize: Boolean? = null
                    var maxFps = "?"
                    try {
                        val caps = ci.getCapabilitiesForType(mime)
                        if (Build.VERSION.SDK_INT >= 30) {
                            ll = caps.isFeatureSupported(MediaCodecInfo.CodecCapabilities.FEATURE_LowLatency)
                        }
                        val vc = caps.videoCapabilities
                        if (vc != null) {
                            okSize = vc.areSizeAndRateSupported(2800, 1752, 120.0)
                            maxFps = try { vc.getSupportedFrameRatesFor(2800, 1752).upper.toInt().toString() } catch (_: Exception) { s(R.string.diag_size_unsupported) }
                        }
                    } catch (_: Exception) {}
                    if (okSize == true) { if (hw) hwOk[label] = true else swOk[label] = true }
                    var vendorTxt = s(R.string.diag_na)
                    if (Build.VERSION.SDK_INT >= 31 && hw && !vendorProbed) {
                        vendorProbed = true
                        var codec: MediaCodec? = null
                        try {
                            codec = MediaCodec.createByCodecName(ci.name)
                            vendorTxt = codec.supportedVendorParameters.size.toString()
                        } catch (_: Throwable) {
                            vendorTxt = s(R.string.diag_unknown)
                        } finally {
                            try { codec?.release() } catch (_: Throwable) {}
                        }
                    }
                    details += s(
                        R.string.diag_codec_item, label, ci.name,
                        if (hw) "HW" else "SW", yn(ll), yn(okSize), maxFps, vendorTxt,
                    )
                }
            }
        } catch (e: Exception) {
            details += s(R.string.diag_error_fmt, e.javaClass.simpleName)
        }
        // App ưu tiên HEVC, rồi H.264; AV1 chỉ khi bật cờ cho phép (xem LowLatencyDecoder).
        val chosen = when {
            hwOk["hevc"] == true -> "HEVC (HW)"
            hwOk["avc"] == true -> "H.264 (HW)"
            swOk["hevc"] == true -> "HEVC (SW)"
            swOk["avc"] == true -> "H.264 (SW)"
            else -> null
        }
        facts.chosenCodec = chosen
        details += kv(R.string.diag_l_chosen_codec, chosen ?: s(R.string.diag_none))
        val title = s(R.string.diag_c_codec)
        return when {
            total == 0 -> CheckResult("codec", title, Verdict.FAIL, s(R.string.diag_codec_none), details,
                s(R.string.diag_codec_none_explain), null)
            chosen == null -> CheckResult("codec", title, Verdict.WARN, s(R.string.diag_codec_nosize), details,
                s(R.string.diag_codec_nosize_explain), s(R.string.diag_codec_nosize_fix))
            chosen.contains("SW") -> CheckResult("codec", title, Verdict.WARN, s(R.string.diag_codec_pick, chosen), details,
                s(R.string.diag_codec_sw_explain), s(R.string.diag_codec_nosize_fix))
            else -> CheckResult("codec", title, Verdict.OK, s(R.string.diag_codec_pick, chosen), details,
                s(R.string.diag_codec_explain), null)
        }
    }

    // ---------------------------------------------------------------- 10. Màn hình
    @Suppress("DEPRECATION")
    fun display(): CheckResult {
        val details = mutableListOf<String>()
        var maxHz = 0f
        try {
            val dm = ctx.getSystemService(Context.DISPLAY_SERVICE) as DisplayManager
            val d = dm.getDisplay(Display.DEFAULT_DISPLAY)
            if (d == null) {
                details += s(R.string.diag_unknown)
            } else {
                val cur = d.mode
                details += kv(R.string.diag_l_cur_mode, "${cur.physicalWidth}x${cur.physicalHeight} @ ${"%.1f".format(Locale.US, cur.refreshRate)} Hz")
                details += kv(R.string.diag_l_cur_refresh, "%.1f Hz".format(Locale.US, d.refreshRate))
                val modes = d.supportedModes.sortedWith(compareBy({ -it.physicalWidth * it.physicalHeight }, { -it.refreshRate }))
                for (m in modes) {
                    maxHz = maxOf(maxHz, m.refreshRate)
                    details += "  ${m.physicalWidth}x${m.physicalHeight} @ ${"%.1f".format(Locale.US, m.refreshRate)} Hz"
                }
                val hdr = d.hdrCapabilities?.supportedHdrTypes ?: IntArray(0)
                val names = hdr.map {
                    when (it) {
                        Display.HdrCapabilities.HDR_TYPE_DOLBY_VISION -> "Dolby Vision"
                        Display.HdrCapabilities.HDR_TYPE_HDR10 -> "HDR10"
                        Display.HdrCapabilities.HDR_TYPE_HLG -> "HLG"
                        Display.HdrCapabilities.HDR_TYPE_HDR10_PLUS -> "HDR10+"
                        else -> it.toString()
                    }
                }
                details += kv(R.string.diag_l_hdr, if (names.isEmpty()) s(R.string.diag_none) else names.joinToString())
            }
        } catch (e: Exception) {
            details += s(R.string.diag_error_fmt, e.javaClass.simpleName)
        }
        val title = s(R.string.diag_c_display)
        return if (maxHz >= 119f) {
            CheckResult("display", title, Verdict.OK, s(R.string.diag_display_120, "%.0f".format(Locale.US, maxHz)), details, s(R.string.diag_display_explain), null)
        } else {
            CheckResult("display", title, Verdict.INFO, s(R.string.diag_display_lower, "%.0f".format(Locale.US, maxHz)), details,
                s(R.string.diag_display_explain), s(R.string.diag_display_fix))
        }
    }

    // ---------------------------------------------------------------- 11. Nhiệt / pin
    fun thermalPower(): CheckResult {
        val details = mutableListOf<String>()
        var warn = false
        val pwr = ctx.getSystemService(Context.POWER_SERVICE) as? PowerManager
        if (pwr != null) {
            if (Build.VERSION.SDK_INT >= 29) {
                val st = pwr.currentThermalStatus
                if (st >= PowerManager.THERMAL_STATUS_MODERATE) warn = true
                details += kv(R.string.diag_l_thermal, thermalName(st))
            }
            if (Build.VERSION.SDK_INT >= 30) {
                val h = try { pwr.getThermalHeadroom(10) } catch (_: Exception) { Float.NaN }
                details += kv(R.string.diag_l_headroom, if (h.isNaN()) s(R.string.diag_unavailable) else "%.2f".format(Locale.US, h))
                if (!h.isNaN() && h >= 0.9f) warn = true
            }
            val ps = pwr.isPowerSaveMode
            if (ps) warn = true
            details += kv(R.string.diag_l_powersave, yn(ps))
        }
        var plugged = 0
        try {
            val b = ctx.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
            if (b != null) {
                val level = b.getIntExtra(BatteryManager.EXTRA_LEVEL, -1)
                val scale = b.getIntExtra(BatteryManager.EXTRA_SCALE, -1)
                plugged = b.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0)
                val status = b.getIntExtra(BatteryManager.EXTRA_STATUS, -1)
                val pct = if (level >= 0 && scale > 0) level * 100 / scale else -1
                details += kv(R.string.diag_l_battery, if (pct >= 0) "$pct%" else "?")
                details += kv(R.string.diag_l_charging, chargingName(status))
                details += kv(R.string.diag_l_plugged, pluggedName(plugged))
                if (plugged == 0 && pct in 0..14) warn = true
            }
        } catch (_: Exception) {}
        val title = s(R.string.diag_c_thermal)
        return CheckResult("thermal", title, if (warn) Verdict.WARN else Verdict.OK,
            if (warn) s(R.string.diag_thermal_warn) else s(R.string.diag_thermal_ok), details,
            s(R.string.diag_thermal_explain), if (warn) s(R.string.diag_thermal_fix) else null)
    }

    private fun thermalName(st: Int): String = when (st) {
        0 -> "NONE"; 1 -> "LIGHT"; 2 -> "MODERATE"; 3 -> "SEVERE"; 4 -> "CRITICAL"; 5 -> "EMERGENCY"; 6 -> "SHUTDOWN"
        else -> st.toString()
    }

    private fun chargingName(status: Int): String = when (status) {
        BatteryManager.BATTERY_STATUS_CHARGING -> s(R.string.diag_bat_charging)
        BatteryManager.BATTERY_STATUS_DISCHARGING -> s(R.string.diag_bat_discharging)
        BatteryManager.BATTERY_STATUS_FULL -> s(R.string.diag_bat_full)
        BatteryManager.BATTERY_STATUS_NOT_CHARGING -> s(R.string.diag_bat_notcharging)
        else -> s(R.string.diag_unknown)
    }

    private fun pluggedName(p: Int): String {
        if (p == 0) return s(R.string.diag_none)
        val l = mutableListOf<String>()
        if (p and BatteryManager.BATTERY_PLUGGED_USB != 0) l += "USB"
        if (p and BatteryManager.BATTERY_PLUGGED_AC != 0) l += "AC"
        if (p and BatteryManager.BATTERY_PLUGGED_WIRELESS != 0) l += "Wireless"
        if (p and 8 != 0) l += "Dock"
        return if (l.isEmpty()) p.toString() else l.joinToString()
    }

    // ---------------------------------------------------------------- 12. Âm thanh
    fun audio(): CheckResult {
        val am = ctx.getSystemService(Context.AUDIO_SERVICE) as? AudioManager
        val fpb = am?.getProperty(AudioManager.PROPERTY_OUTPUT_FRAMES_PER_BUFFER)
        val rate = am?.getProperty(AudioManager.PROPERTY_OUTPUT_SAMPLE_RATE)
        val ll = pm.hasSystemFeature(PackageManager.FEATURE_AUDIO_LOW_LATENCY)
        val pro = pm.hasSystemFeature(PackageManager.FEATURE_AUDIO_PRO)
        val details = listOf(
            kv(R.string.diag_l_fpb, fpb ?: s(R.string.diag_unknown)),
            kv(R.string.diag_l_rate, rate ?: s(R.string.diag_unknown)),
            kv(R.string.diag_l_audio_ll, yn(ll)),
            kv(R.string.diag_l_audio_pro, yn(pro)),
        )
        val title = s(R.string.diag_c_audio)
        return if (ll) {
            CheckResult("audio", title, Verdict.OK, s(R.string.diag_audio_ok), details, s(R.string.diag_audio_explain), null)
        } else {
            CheckResult("audio", title, Verdict.WARN, s(R.string.diag_audio_nolow), details, s(R.string.diag_audio_explain), s(R.string.diag_audio_fix))
        }
    }
}
