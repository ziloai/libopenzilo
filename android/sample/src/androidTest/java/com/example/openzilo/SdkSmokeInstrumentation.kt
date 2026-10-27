// SPDX-License-Identifier: MPL-2.0
package com.example.openzilo

import android.app.Activity
import android.app.Instrumentation
import android.bluetooth.BluetoothManager
import android.os.Bundle
import com.zilo.libopenzilo.*
import com.zilo.sdk.OpenZiloDevice
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit

/**
 * Explicit opt-in hardware check: reads metadata, subscribes briefly to IMU, never
 * records/deletes/updates.
 */
class SdkSmokeInstrumentation : Instrumentation() {
    private lateinit var arguments: Bundle

    override fun onCreate(arguments: Bundle) {
        super.onCreate(arguments)
        this.arguments = arguments
        start()
    }

    override fun onStart() {
        val output = Bundle()
        var ring: OpenZiloDevice? = null
        var imu = false
        var stage = "connect"
        try {
            val address =
                arguments.getString("address")
                    ?: error("Provide -e address with a ring selected by the developer")
            val device =
                targetContext
                    .getSystemService(BluetoothManager::class.java)
                    .adapter
                    .getRemoteDevice(address)
            ring = OpenZiloDevice.connect(targetContext, device).get(25, TimeUnit.SECONDS)
            stage = "system-info"
            val info = ring.protocol.systemInfo().get(15, TimeUnit.SECONDS)
            check(info.model.isNotBlank())
            check(info.batteryPercent.toInt() in 0..100)
            stage = "capabilities"
            val caps = ring.protocol.discoverCapabilities().get(15, TimeUnit.SECONDS)
            check(caps.firmwareVersion == info.firmwareVersion && caps.model == info.model)
            stage = "audio-count"
            val count = ring.protocol.audioCount().get(15, TimeUnit.SECONDS)
            stage = "audio-info"
            if (count > 0) {
                val audio = ring.protocol.audioInfo(0).get(15, TimeUnit.SECONDS)
                if (arguments.getString("download") == "true" && audio.dataSize <= 1024u * 1024u) {
                    stage = "audio-download"
                    val data = ring.protocol.downloadAudio(0, 1024 * 1024).get(120, TimeUnit.SECONDS)
                    check(data.size.toUInt() == audio.dataSize)
                    check(ring.protocol.audioCount().get(15, TimeUnit.SECONDS) == count)
                    output.putString("audio_download", "PASS: exact size and unchanged recording count")
                }
            }
            stage = "recording-config"
            ring.protocol.recordingConfig().get(15, TimeUnit.SECONDS)
            val report = CompletableFuture<ImuBatch>()
            val listener =
                ring.protocol.addPacketListener {
                    if (it.command.toInt() == 0x605)
                        try {
                            report.complete(parseImu(it.body))
                        } catch (e: Throwable) {
                            report.completeExceptionally(e)
                        }
                }
            try {
                stage = "imu-start"
                ring.protocol.startImu().get(15, TimeUnit.SECONDS)
                imu = true
                stage = "imu-report"
                check(report.get(15, TimeUnit.SECONDS).samples.isNotEmpty())
                stage = "imu-stop"
                ring.protocol.stopImu().get(15, TimeUnit.SECONDS)
                imu = false
            } finally {
                listener.close()
            }
            output.putString(
                "result",
                "PASS: native load, BLE connect/fragmented info, capabilities, recording metadata/config, IMU start/data/stop",
            )
            output.putString("core_version", coreVersion())
            finish(Activity.RESULT_OK, output)
        } catch (e: Throwable) {
            output.putString("result", "FAIL at $stage: ${e.javaClass.simpleName}: ${e.message}")
            finish(Activity.RESULT_CANCELED, output)
        } finally {
            if (imu)
                try {
                    ring?.protocol?.stopImu()?.get(3, TimeUnit.SECONDS)
                } catch (_: Throwable) {}
            ring?.close()
        }
    }
}
