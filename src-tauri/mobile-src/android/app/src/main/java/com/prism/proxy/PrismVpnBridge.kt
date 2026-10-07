package com.prism.proxy

import android.content.ComponentName
import android.content.Intent
import android.net.VpnService
import android.os.Build
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

// Prism VPN 桥：Rust（JNI）唯一入口。
//
// 生命周期顺序（Rust 侧保证）：
//   ensurePermission() → startServiceAndWait() → [libbox openTun/protect] → stopService()
object PrismVpnBridge {
    private const val TAG = "PrismVpn"

    @Volatile
    private var activity: ComponentActivity? = null
    private var consentLauncher: ActivityResultLauncher<Intent>? = null
    @Volatile
    private var consentInFlight = false

    @Volatile
    private var service: PrismVpnService? = null
    @Volatile
    private var ready = CountDownLatch(1)

    @JvmStatic
    external fun nativeVpnRevoked()

    // 由 MainActivity 的 init 块调用（必须早于 Activity STARTED，
    // 因为 registerForActivityResult 要求注册在启动之前）
    fun attach(activity: ComponentActivity) {
        this.activity = activity
        this.consentLauncher = activity.registerForActivityResult(
            ActivityResultContracts.StartActivityForResult()
        ) { result ->
            consentInFlight = false
            Log.i(TAG, "vpn consent result=${result.resultCode}")
        }
    }

    // 系统 VPN 授权检查：
    //   true  —— 已授权（或此前已授权，VpnService.prepare 返回 null）
    //   false —— 已发起系统授权弹窗，用户授权后需重新点击启动
    @JvmStatic
    fun ensurePermission(): Boolean {
        val activity = this.activity ?: return false
        val intent = VpnService.prepare(activity) ?: return true
        if (consentInFlight) return false
        consentInFlight = true
        return try {
            consentLauncher?.launch(intent)
            false
        } catch (e: Exception) {
            Log.e(TAG, "launch consent failed", e)
            consentInFlight = false
            false
        }
    }

    // 启动前台 VPN 服务并阻塞等待其就绪（Go 索要 TUN fd 前服务必须已前台化）
    @JvmStatic
    fun startServiceAndWait(): Boolean {
        val activity = this.activity ?: return false
        ready = CountDownLatch(1)
        service = null
        val intent = Intent().apply { component = ComponentName(activity, PrismVpnService::class.java) }
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                activity.applicationContext.startForegroundService(intent)
            } else {
                activity.applicationContext.startService(intent)
            }
        } catch (e: Exception) {
            Log.e(TAG, "start vpn service failed", e)
            return false
        }
        ready.await(8, TimeUnit.SECONDS)
        return service != null
    }

    fun onServiceReady(service: PrismVpnService) {
        this.service = service
        ready.countDown()
    }

    // 停止并销毁前台 VPN 服务
    @JvmStatic
    fun stopService() {
        val activity = this.activity
        try {
            service?.shutdown()
        } catch (e: Exception) {
            Log.w(TAG, "service shutdown error", e)
        }
        if (activity != null) {
            val intent = Intent().apply {
                component = ComponentName(activity, PrismVpnService::class.java)
            }
            try {
                activity.applicationContext.stopService(intent)
            } catch (_: Exception) {
            }
        }
        service = null
    }

    @JvmStatic
    fun openTun(optionsJson: String): Int {
        val service = this.service ?: run {
            Log.e(TAG, "openTun: service not ready")
            return -1
        }
        return try {
            service.openTun(optionsJson)
        } catch (e: Exception) {
            Log.e(TAG, "openTun failed", e)
            -1
        }
    }

    @JvmStatic
    fun protectSocket(fd: Int): Boolean {
        val service = this.service ?: return false
        return try {
            service.protect(fd)
        } catch (e: Exception) {
            Log.e(TAG, "protect($fd) failed", e)
            false
        }
    }
}
