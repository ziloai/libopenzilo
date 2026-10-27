// SPDX-License-Identifier: MPL-2.0
package com.zilo.sdk

import android.bluetooth.BluetoothDevice
import android.content.Context
import java.util.UUID
import java.util.concurrent.CompletableFuture
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlinx.coroutines.suspendCancellableCoroutine

/** Java-friendly view; advanced Kotlin APIs expose com.zilo.core records directly. */
data class RingInfo(
    val firmwareVersion: String,
    val model: String,
    val batteryPercent: Int,
    val charging: Boolean,
)

data class FirmwareUpdateResult(val device: OpenZiloDevice, val info: RingInfo)

class OpenZiloDevice
private constructor(
    private val context: Context,
    private val peer: BluetoothDevice,
    private val transport: AndroidBleTransport,
) : AutoCloseable {
    val protocol = ProtocolClient(transport)

    fun hasService(uuid: UUID) = transport.hasService(uuid)

    fun getSystemInfo(): CompletableFuture<RingInfo> =
        protocol.systemInfo().mapResult {
            RingInfo(it.firmwareVersion, it.model, it.batteryPercent.toInt(), it.batteryCharging)
        }

    /** Complete signed update: transfer, reconnect the same ring, verify exact running version. */
    @JvmOverloads
    fun updateFirmware(
        bundle: ByteArray,
        trustedPublicKey: String,
        allowTest: Boolean = false,
    ): CompletableFuture<FirmwareUpdateResult> {
        val result = CompletableFuture<FirmwareUpdateResult>()
        val reconnect =
            java.util.concurrent.Executors.newSingleThreadScheduledExecutor { r ->
                Thread(r, "OpenZilo-reconnect").apply { isDaemon = true }
            }
        val attempt =
            java.util.concurrent.atomic.AtomicReference<CompletableFuture<OpenZiloDevice>?>(null)
        val candidate = java.util.concurrent.atomic.AtomicReference<OpenZiloDevice?>(null)
        val state = java.util.concurrent.atomic.AtomicReference<com.zilo.core.OtaTransfer?>(null)
        // Verify signature before performing device operations.
        try {
            com.zilo.core.inspectFirmwareBundle(bundle, trustedPublicKey)
        } catch (e: Throwable) {
            reconnect.shutdown()
            result.completeExceptionally(e)
            return result
        }
        val pipeline =
            protocol.discoverCapabilities().flatMapResult { caps ->
                protocol.recordingStatus().flatMapResult { status ->
                    if (status.state.toInt() in 1..3)
                        throw RingException("busy", "Device is recording")
                    protocol.systemInfo().flatMapResult { before ->
                        protocol
                            .transferFirmware(
                                bundle,
                                trustedPublicKey,
                                before,
                                caps.boardTarget,
                                allowTest,
                            )
                            .mapResult { Pair(before, it) }
                    }
                }
            }
        pipeline.whenComplete { transfer, error ->
            if (error != null) result.completeExceptionally(error)
            else {
                if (result.isDone) {
                    transfer.second.close()
                    close()
                    return@whenComplete
                }
                state.set(transfer.second)
                close()
                fun retry() {
                    if (result.isDone) return
                    val connection = connect(context, peer)
                    attempt.set(connection)
                    connection.whenComplete { ring, connectError ->
                        if (result.isDone) {
                            ring?.close()
                            return@whenComplete
                        }
                        if (connectError != null) {
                            reconnect.schedule(
                                { retry() },
                                1000,
                                java.util.concurrent.TimeUnit.MILLISECONDS,
                            )
                            return@whenComplete
                        }
                        candidate.set(ring)
                        ring.protocol.systemInfo().whenComplete { after, readError ->
                            if (result.isDone) {
                                ring.close()
                                return@whenComplete
                            }
                            if (
                                readError != null ||
                                    after.firmwareVersion == transfer.first.firmwareVersion
                            ) {
                                ring.close()
                                candidate.compareAndSet(ring, null)
                                reconnect.schedule(
                                    { retry() },
                                    1000,
                                    java.util.concurrent.TimeUnit.MILLISECONDS,
                                )
                            } else
                                try {
                                    transfer.second.verifyRunning(after)
                                    val info =
                                        RingInfo(
                                            after.firmwareVersion,
                                            after.model,
                                            after.batteryPercent.toInt(),
                                            after.batteryCharging,
                                        )
                                    if (result.complete(FirmwareUpdateResult(ring, info)))
                                        candidate.compareAndSet(ring, null)
                                    else ring.close()
                                } catch (e: Throwable) {
                                    ring.close()
                                    result.completeExceptionally(e)
                                }
                        }
                    }
                }
                reconnect.schedule({ retry() }, 1000, java.util.concurrent.TimeUnit.MILLISECONDS)
                reconnect.schedule(
                    {
                        result.completeExceptionally(
                            RingException(
                                "timeout",
                                "Firmware installation is unconfirmed after reconnect timeout",
                            )
                        )
                    },
                    60000,
                    java.util.concurrent.TimeUnit.MILLISECONDS,
                )
            }
        }
        result.whenComplete { _, error ->
            if (error != null) {
                pipeline.cancel(false)
                attempt.get()?.cancel(false)
                candidate.getAndSet(null)?.close()
                close()
            }
            state.getAndSet(null)?.close()
            reconnect.shutdownNow()
        }
        return result
    }

    override fun close() = protocol.close()

    companion object {
        @JvmStatic
        fun connect(context: Context, device: BluetoothDevice): CompletableFuture<OpenZiloDevice> {
            val ring =
                OpenZiloDevice(
                    context.applicationContext,
                    device,
                    AndroidBleTransport(context.applicationContext, device),
                )
            val result = CompletableFuture<OpenZiloDevice>()
            ring.protocol.ready.whenComplete { _, error ->
                if (error != null) {
                    ring.close()
                    result.completeExceptionally(error)
                } else if (!result.complete(ring)) ring.close()
            }
            result.whenComplete { _, _ -> if (result.isCancelled) ring.close() }
            return result
        }
    }
}

/** Cancellation propagates to the SDK request; it never retries a state-changing command. */
suspend fun <T> CompletableFuture<T>.awaitResult(): T =
    suspendCancellableCoroutine { continuation ->
        whenComplete { value, error ->
            if (continuation.isActive) {
                if (error != null) continuation.resumeWithException(error)
                else continuation.resume(value)
            }
        }
        continuation.invokeOnCancellation { cancel(false) }
    }

/** Future combinators retain cancellation across typed high-level operations. */
internal fun <T, R> CompletableFuture<T>.mapResult(transform: (T) -> R): CompletableFuture<R> {
    val result = CompletableFuture<R>()
    whenComplete { value, error ->
        if (error != null) result.completeExceptionally(error)
        else if (!result.isDone)
            try {
                result.complete(transform(value))
            } catch (e: Throwable) {
                result.completeExceptionally(e)
            }
    }
    result.whenComplete { _, _ -> if (result.isCancelled) cancel(false) }
    return result
}

internal fun <T, R> CompletableFuture<T>.flatMapResult(
    transform: (T) -> CompletableFuture<R>
): CompletableFuture<R> {
    val result = CompletableFuture<R>()
    val child = java.util.concurrent.atomic.AtomicReference<CompletableFuture<R>?>(null)
    whenComplete { value, error ->
        if (error != null) result.completeExceptionally(error)
        else if (!result.isDone)
            try {
                val next = transform(value)
                child.set(next)
                if (result.isCancelled) next.cancel(false)
                next.whenComplete { item, failure ->
                    if (failure != null) result.completeExceptionally(failure)
                    else result.complete(item)
                }
            } catch (e: Throwable) {
                result.completeExceptionally(e)
            }
    }
    result.whenComplete { _, _ ->
        if (result.isCancelled) {
            cancel(false)
            child.get()?.cancel(false)
        }
    }
    return result
}
