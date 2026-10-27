// SPDX-License-Identifier: MPL-2.0
package com.example.openzilo

import android.Manifest
import android.app.Activity
import android.bluetooth.*
import android.bluetooth.le.*
import android.content.pm.PackageManager
import android.os.*
import android.widget.*
import com.zilo.sdk.*

/** Standalone third-party app: discovers candidates, then reads info and capabilities. */
class MainActivity : Activity() {
    private lateinit var output: LinearLayout
    private var scanner: BluetoothLeScanner? = null
    private var ring: OpenZiloDevice? = null
    private val peers = mutableSetOf<String>()
    private val handler = Handler(Looper.getMainLooper())

    private fun line(text: String) {
        output.addView(
            TextView(this).apply {
                this.text = text
                setPadding(16, 16, 16, 16)
            }
        )
    }

    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        output = LinearLayout(this).apply { orientation = 1 }
        setContentView(ScrollView(this).apply { addView(output) })
        output.addView(
            Button(this).apply {
                text = "扫描 OpenZilo"
                setOnClickListener { scan() }
            }
        )
        line("选择戒指后读取型号、电量、固件和能力。不会修改或删除数据。")
    }

    private fun scan() {
        val permissions =
            if (Build.VERSION.SDK_INT >= 31)
                arrayOf(Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_CONNECT)
            else arrayOf(Manifest.permission.ACCESS_FINE_LOCATION)
        if (permissions.any { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }) {
            requestPermissions(permissions, 1)
            return
        }
        scanner?.stopScan(callback)
        peers.clear()
        val adapter = getSystemService(BluetoothManager::class.java)?.adapter
        if (adapter?.isEnabled != true) {
            line("请开启蓝牙")
            return
        }
        scanner = adapter.bluetoothLeScanner
        scanner?.startScan(
            listOf(
                ScanFilter.Builder()
                    .setServiceUuid(
                        ParcelUuid(
                            java.util.UUID.fromString("6e400001-b5a3-f393-e0a9-e50e24dcca9e")
                        )
                    )
                    .build()
            ),
            ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build(),
            callback,
        )
        line("扫描中…")
        handler.postDelayed({ scanner?.stopScan(callback) }, 15000)
    }

    private val callback: ScanCallback =
        object : ScanCallback() {
            override fun onScanResult(type: Int, result: ScanResult) {
                runOnUiThread {
                    if (!peers.add(result.device.address)) return@runOnUiThread
                    output.addView(
                        Button(this@MainActivity).apply {
                            text = result.scanRecord?.deviceName ?: "OpenZilo candidate"
                            setOnClickListener {
                                scanner?.stopScan(callback)
                                ring?.close()
                                line("连接中…")
                                OpenZiloDevice.connect(this@MainActivity, result.device)
                                    .whenComplete { device, error ->
                                        if (error != null)
                                            runOnUiThread { line("连接失败：${error.message}") }
                                        else {
                                            ring = device
                                            device
                                                .getSystemInfo()
                                                .thenCompose { info ->
                                                    runOnUiThread {
                                                        line(
                                                            "${info.model} / ${info.firmwareVersion} / ${info.batteryPercent}%"
                                                        )
                                                    }
                                                    device.protocol.discoverCapabilities()
                                                }
                                                .whenComplete { caps, err ->
                                                    runOnUiThread {
                                                        line(
                                                            if (err != null) "能力不可用：${err.message}"
                                                            else
                                                                "共享核心：${com.zilo.libopenzilo.coreVersion()}\n能力：${caps.features.joinToString { it.name }}"
                                                        )
                                                    }
                                                }
                                        }
                                    }
                            }
                        }
                    )
                }
            }

            override fun onScanFailed(code: Int) {
                runOnUiThread { line("扫描失败：$code") }
            }
        }

    override fun onDestroy() {
        handler.removeCallbacksAndMessages(null)
        scanner?.stopScan(callback)
        ring?.close()
        super.onDestroy()
    }
}
