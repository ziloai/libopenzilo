// SPDX-License-Identifier: MPL-2.0
package com.zilo.sdk

import android.annotation.SuppressLint
import android.bluetooth.*
import android.content.Context
import android.os.Handler
import android.os.Looper
import java.util.ArrayDeque
import java.util.UUID
import java.util.concurrent.CompletableFuture

/** Android GATT adapter. One serialized operation queue; an interrupted frame closes the link. */
@SuppressLint("MissingPermission")
internal class AndroidBleTransport(
    private val context: Context,
    private val device: BluetoothDevice,
) : RingTransport {
    private val handler = Handler(Looper.getMainLooper())
    private val ready = CompletableFuture<Void>()
    private var gatt: BluetoothGatt? = null
    private var rx: BluetoothGattCharacteristic? = null
    private var receiver: ((ByteArray) -> Unit)? = null
    private var disconnected: ((Throwable) -> Unit)? = null
    private var closed = false
    private var chunkSize = 20
    private var writing = false
    private var subscribing = false
    private var offset = 0
    private val queue = ArrayDeque<Pair<ByteArray, CompletableFuture<Void>>>()
    private var services: Set<UUID> = emptySet()
    private val connectTimeout = Runnable {
        fail(RingException("timeout", "BLE connection setup timed out"))
    }
    private val writeTimeout = Runnable {
        fail(RingException("timeout", "BLE write was interrupted"))
    }

    fun hasService(uuid: UUID) = services.contains(uuid)

    override fun start(
        receiver: (ByteArray) -> Unit,
        disconnected: (Throwable) -> Unit,
    ): CompletableFuture<Void> {
        this.receiver = receiver
        this.disconnected = disconnected
        handler.post {
            if (closed) {
                ready.completeExceptionally(RingException("disconnected", "Connection closed"))
                return@post
            }
            handler.postDelayed(connectTimeout, 20000)
            try {
                gatt =
                    device.connectGatt(
                        context.applicationContext,
                        false,
                        callback,
                        BluetoothDevice.TRANSPORT_LE,
                    )
                if (gatt == null) fail(RingException("transport", "connectGatt returned null"))
            } catch (e: Throwable) {
                fail(e)
            }
        }
        return ready
    }

    private fun current(connection: BluetoothGatt) = !closed && connection === gatt

    private fun guarded(connection: BluetoothGatt, block: () -> Unit) {
        handler.post {
            if (current(connection))
                try {
                    block()
                } catch (e: Throwable) {
                    fail(e)
                }
        }
    }

    private val callback =
        object : BluetoothGattCallback() {
            override fun onConnectionStateChange(
                connection: BluetoothGatt,
                status: Int,
                state: Int,
            ) =
                guarded(connection) {
                    if (
                        status != BluetoothGatt.GATT_SUCCESS ||
                            state == BluetoothProfile.STATE_DISCONNECTED
                    )
                        fail(RingException("disconnected", "BLE disconnected ($status)"))
                    else if (
                        state == BluetoothProfile.STATE_CONNECTED && !connection.discoverServices()
                    )
                        fail(RingException("transport", "Service discovery did not start"))
                }

            override fun onServicesDiscovered(connection: BluetoothGatt, status: Int) =
                guarded(connection) {
                    if (status != 0) {
                        fail(RingException("transport", "Service discovery failed ($status)"))
                        return@guarded
                    }
                    services = connection.services.map { it.uuid }.toSet()
                    val service = connection.getService(NUS)
                    rx = service?.getCharacteristic(RX)
                    if (rx == null || service.getCharacteristic(TX) == null) {
                        fail(RingException("incompatible", "Device has no OpenZilo NUS service"))
                        return@guarded
                    }
                    if (!connection.requestMtu(498)) enableNotifications(connection)
                }

            override fun onMtuChanged(connection: BluetoothGatt, mtu: Int, status: Int) =
                guarded(connection) {
                    if (status == 0) chunkSize = (mtu - 3).coerceIn(20, 495)
                    if (!ready.isDone) enableNotifications(connection)
                }

            override fun onDescriptorWrite(
                connection: BluetoothGatt,
                descriptor: BluetoothGattDescriptor,
                status: Int,
            ) =
                guarded(connection) {
                    if (descriptor.uuid != CCCD) return@guarded
                    if (status != 0) {
                        fail(
                            RingException("transport", "Notification subscription failed ($status)")
                        )
                        return@guarded
                    }
                    handler.removeCallbacks(connectTimeout)
                    ready.complete(null)
                    pump()
                }

            @Deprecated("Legacy callback for Android 26..32")
            override fun onCharacteristicChanged(
                connection: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
            ) {
                val data = characteristic.value?.clone() ?: return
                notification(connection, characteristic.uuid, data)
            }

            override fun onCharacteristicChanged(
                connection: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                value: ByteArray,
            ) {
                notification(connection, characteristic.uuid, value.clone())
            }

            override fun onCharacteristicWrite(
                connection: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                status: Int,
            ) =
                guarded(connection) {
                    if (characteristic.uuid != RX || !writing) return@guarded
                    handler.removeCallbacks(writeTimeout)
                    if (status != 0) {
                        fail(RingException("transport", "BLE write failed ($status)"))
                        return@guarded
                    }
                    writing = false
                    val item = queue.peekFirst() ?: return@guarded
                    if (offset >= item.first.size) {
                        queue.removeFirst()
                        offset = 0
                        item.second.complete(null)
                    }
                    pump()
                }
        }

    private fun notification(connection: BluetoothGatt, uuid: UUID, data: ByteArray) {
        if (uuid == TX) guarded(connection) { receiver?.invoke(data) }
    }

    @Suppress("DEPRECATION")
    private fun enableNotifications(connection: BluetoothGatt) {
        if (subscribing || ready.isDone) return
        subscribing = true
        val tx =
            connection.getService(NUS)?.getCharacteristic(TX)
                ?: throw RingException("incompatible", "NUS TX is unavailable")
        val descriptor =
            tx.getDescriptor(CCCD) ?: throw RingException("incompatible", "NUS CCCD is unavailable")
        if (!connection.setCharacteristicNotification(tx, true))
            throw RingException("transport", "Cannot enable notifications")
        descriptor.value = BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE
        if (!connection.writeDescriptor(descriptor))
            throw RingException("transport", "Cannot write notification descriptor")
    }

    override fun send(frame: ByteArray): CompletableFuture<Void> {
        val future = CompletableFuture<Void>()
        handler.post {
            if (closed) {
                future.completeExceptionally(RingException("disconnected", "BLE connection closed"))
                return@post
            }
            if (queue.size >= 128) {
                future.completeExceptionally(RingException("overflow", "BLE send queue is full"))
                return@post
            }
            queue.addLast(Pair(frame.clone(), future))
            pump()
        }
        return future
    }

    @Suppress("DEPRECATION")
    private fun pump() {
        if (closed || !ready.isDone || ready.isCompletedExceptionally || writing || queue.isEmpty())
            return
        val connection = gatt ?: return
        val characteristic = rx ?: return
        val data = queue.first.first
        if (data.isEmpty()) {
            queue.removeFirst().second.complete(null)
            pump()
            return
        }
        val end = (offset + chunkSize).coerceAtMost(data.size)
        characteristic.writeType = BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE
        characteristic.value = data.copyOfRange(offset, end)
        writing = true
        offset = end
        try {
            handler.postDelayed(writeTimeout, 10000)
            if (!connection.writeCharacteristic(characteristic))
                fail(RingException("transport", "BLE write did not start"))
        } catch (e: Throwable) {
            fail(e)
        }
    }

    private fun fail(error: Throwable) {
        if (closed) return
        closed = true
        handler.removeCallbacks(connectTimeout)
        handler.removeCallbacks(writeTimeout)
        ready.completeExceptionally(error)
        while (queue.isNotEmpty()) queue.removeFirst().second.completeExceptionally(error)
        val previous = gatt
        gatt = null
        try {
            previous?.disconnect()
        } catch (_: RuntimeException) {}
        try {
            previous?.close()
        } catch (_: RuntimeException) {}
        disconnected?.invoke(error)
    }

    override fun close() {
        handler.post { fail(RingException("disconnected", "BLE connection closed")) }
    }

    companion object {
        val NUS: UUID = UUID.fromString("6e400001-b5a3-f393-e0a9-e50e24dcca9e")
        private val RX = UUID.fromString("6e400002-b5a3-f393-e0a9-e50e24dcca9e")
        private val TX = UUID.fromString("6e400003-b5a3-f393-e0a9-e50e24dcca9e")
        private val CCCD = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")
    }
}
