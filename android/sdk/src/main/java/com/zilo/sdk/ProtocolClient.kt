// SPDX-License-Identifier: MPL-2.0
package com.zilo.sdk

import com.zilo.core.*
import java.util.concurrent.*
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow

/** A transport sends whole frames, serializes writes, and reports any partial-write failure. */
interface RingTransport : AutoCloseable {
    fun start(
        receiver: (ByteArray) -> Unit,
        disconnected: (Throwable) -> Unit,
    ): CompletableFuture<Void>

    fun send(frame: ByteArray): CompletableFuture<Void>
}

class RingException(val kind: String, message: String, val deviceCode: Int? = null) :
    RuntimeException(message)

/** Platform-independent async client. Each instance owns one transport connection. */
class ProtocolClient
@JvmOverloads
constructor(private val transport: RingTransport, private val timeoutMs: Long = 10000) :
    AutoCloseable {
    private val executor =
        Executors.newSingleThreadScheduledExecutor { r ->
            Thread(r, "OpenZilo-session").apply { isDaemon = true }
        }
    private val session = DeviceSession(256u)
    private val generation = session.open()
    private val pending = mutableMapOf<ULong, CompletableFuture<Packet>>()
    private val listeners = CopyOnWriteArrayList<(Packet) -> Unit>()
    private val closeListeners = CopyOnWriteArrayList<(Throwable) -> Unit>()
    private val closed = AtomicBoolean(false)
    val ready: CompletableFuture<Void> = CompletableFuture()
    @Volatile
    var capabilities: Capabilities? = null
        private set

    @Volatile private var recordingActive = false
    @Volatile private var liveSubscribed = false

    init {
        require(timeoutMs > 0)
        executor.scheduleAtFixedRate(
            { if (!closed.get()) dispatch(session.tick(now())) },
            100,
            100,
            TimeUnit.MILLISECONDS,
        )
        try {
            transport
                .start(
                    { bytes ->
                        enqueue {
                            try {
                                dispatch(session.receive(bytes, generation, now()))
                                session.takeEvents()
                            } catch (e: Throwable) {
                                fail(e)
                            }
                        }
                    },
                    { error -> enqueue { fail(error) } },
                )
                .whenComplete { _, error ->
                    if (error != null) enqueue { fail(error) } else ready.complete(null)
                }
        } catch (e: Throwable) {
            enqueue { fail(e) }
        }
    }

    private fun now() = TimeUnit.NANOSECONDS.toMillis(System.nanoTime()).toULong()

    private fun enqueue(block: () -> Unit) {
        try {
            executor.execute(block)
        } catch (_: RejectedExecutionException) {}
    }

    private fun <T> launch(result: CompletableFuture<T>, block: () -> Unit) {
        if (result.isDone) return
        try {
            executor.execute {
                if (result.isDone) return@execute
                if (closed.get())
                    result.completeExceptionally(
                        RingException("disconnected", "Connection is closed")
                    )
                else
                    try {
                        block()
                    } catch (e: Throwable) {
                        result.completeExceptionally(e)
                    }
            }
        } catch (_: RejectedExecutionException) {
            result.completeExceptionally(RingException("disconnected", "Connection is closed"))
        }
    }

    private fun dispatch(items: List<Delivery>) {
        for (item in items) when (item) {
            is Delivery.Response -> pending.remove(item.requestId)?.complete(item.packet)
            is Delivery.Failure ->
                pending
                    .remove(item.requestId)
                    ?.completeExceptionally(
                        RingException(item.kind, item.message, item.deviceCode?.toInt())
                    )
            is Delivery.Event ->
                listeners.forEach { listener ->
                    try {
                        listener(item.packet)
                    } catch (e: Throwable) {
                        fail(e)
                        return
                    }
                }
        }
    }

    /** Events are provisional; a slow Flow consumer fails rather than silently dropping packets. */
    fun events(): Flow<Packet> = callbackFlow {
        val listener: (Packet) -> Unit = {
            if (trySend(it).isFailure)
                close(RingException("overflow", "Event consumer is too slow"))
        }
        val ended: (Throwable) -> Unit = { close(it) }
        listeners.add(listener)
        closeListeners.add(ended)
        if (closed.get()) close(RingException("disconnected", "Connection is closed"))
        awaitClose {
            listeners.remove(listener)
            closeListeners.remove(ended)
        }
    }

    fun addPacketListener(listener: (Packet) -> Unit): AutoCloseable {
        check(!closed.get())
        listeners.add(listener)
        return AutoCloseable { listeners.remove(listener) }
    }

    @JvmOverloads
    fun request(request: DeviceRequest, timeout: Long = timeoutMs): CompletableFuture<Packet> =
        perform(prepareRequest(request), timeout, null)

    internal fun owned(request: DeviceRequest, owner: String): CompletableFuture<Packet> =
        perform(prepareRequest(request), timeoutMs, owner)

    private fun perform(
        spec: RequestSpec,
        timeout: Long,
        owner: String?,
    ): CompletableFuture<Packet> {
        val result = CompletableFuture<Packet>()
        if (closed.get()) {
            result.completeExceptionally(RingException("disconnected", "Connection is closed"))
            return result
        }
        ready.whenComplete { _, error ->
            if (error != null) result.completeExceptionally(error)
            else
                launch(result) {
                    if (result.isCancelled) return@launch
                    try {
                        require(timeout > 0)
                        val enhanced =
                            spec.feature in
                                setOf("recording_control", "live_audio", "imu", "gesture", "ota")
                        if (enhanced) {
                            val caps =
                                capabilities
                                    ?: throw RingException(
                                        "incompatible",
                                        "Discover capabilities before enhanced operations",
                                    )
                            val major = if (spec.feature == "imu") 2u else 1u
                            if (
                                !supportsFeature(caps.features, spec.feature!!, major.toUByte(), 0u)
                            )
                                throw RingException(
                                    "incompatible",
                                    "Required device contract ${spec.feature} is unavailable",
                                )
                        }
                        val response = spec.response
                        if (response == null) {
                            val bytes = session.prepareSend(spec.command, spec.body, owner)
                            transport.send(bytes).whenComplete { _, failure ->
                                if (failure != null) {
                                    result.completeExceptionally(failure)
                                    enqueue { fail(failure) }
                                } else result.complete(Packet(spec.command, byteArrayOf(), 4u, 0u))
                            }
                        } else {
                            val ticket =
                                session.begin(
                                    spec.command,
                                    response,
                                    spec.body,
                                    now(),
                                    timeout.toULong(),
                                    owner,
                                )
                            pending[ticket.id] = result
                            result.whenComplete { _, _ ->
                                if (result.isCancelled)
                                    enqueue {
                                        session.cancel(ticket.id)
                                        pending.remove(ticket.id)
                                    }
                            }
                            transport.send(ticket.frame).whenComplete { _, failure ->
                                if (failure != null)
                                    enqueue {
                                        session.interruptWrite(generation)
                                        fail(failure)
                                    }
                            }
                        }
                    } catch (e: Throwable) {
                        result.completeExceptionally(e)
                    }
                }
        }
        return result
    }

    fun systemInfo(): CompletableFuture<DeviceInfo> =
        request(DeviceRequest.SystemInfo).mapResult { parseSystemInfo(it.body) }

    fun discoverCapabilities(): CompletableFuture<Capabilities> =
        systemInfo().flatMapResult { info ->
            request(DeviceRequest.Capabilities).mapResult {
                val caps = parseCapabilities(it.body)
                if (caps.firmwareVersion != info.firmwareVersion || caps.model != info.model)
                    throw RingException("protocol", "Capability identity differs from system info")
                capabilities = caps
                caps
            }
        }

    fun audioCount(): CompletableFuture<Long> =
        request(DeviceRequest.AudioCount).mapResult { parseAudioCount(it.body).toLong() }

    fun audioInfo(index: Long): CompletableFuture<AudioFileInfo> {
        require(index in 0..0xffffffffL)
        val result = CompletableFuture<AudioFileInfo>()
        val owner = "audio-info-${System.nanoTime()}"
        launch(result) {
            try {
                session.acquireExclusive(owner)
            } catch (e: Throwable) {
                result.completeExceptionally(e)
                return@launch
            }
            val operation =
                owned(DeviceRequest.AudioInfo(index.toUInt()), owner).flatMapResult { packet ->
                    val info = parseAudioInfo(packet.body)
                    if (info.fileIndex != index.toUInt())
                        throw RingException("protocol", "Audio file identity differs")
                    owned(DeviceRequest.AudioEnd(index.toUInt()), owner).mapResult { ack ->
                        if (parseAudioEnd(ack.body) != index.toUInt())
                            throw RingException("protocol", "Audio completion identity differs")
                        info
                    }
                }
            result.whenComplete { _, error ->
                enqueue {
                    if (error != null) {
                        operation.cancel(false)
                        fail(error)
                    }
                }
            }
            operation.whenComplete { info, error ->
                enqueue {
                    if (error != null) {
                        fail(error)
                        result.completeExceptionally(error)
                    } else {
                        session.releaseExclusive(owner)
                        result.complete(info)
                    }
                }
            }
        }
        return result
    }

    fun recordingConfig(): CompletableFuture<RecordingConfig> =
        request(DeviceRequest.GetRecordingConfig).mapResult { parseRecordingConfig(it.body) }

    fun setRecordingConfig(config: RecordingConfig): CompletableFuture<RecordingConfig> {
        capabilities?.recordingLimits?.let { validateRecordingConfig(config, it) }
        return request(DeviceRequest.SetRecordingConfig(config)).mapResult {
            parseRecordingConfig(it.body).also { applied ->
                if (applied != config)
                    throw RingException("protocol", "Configuration readback differs")
            }
        }
    }

    fun startRecording(): CompletableFuture<Long> {
        recordingActive = true
        return request(DeviceRequest.StartRecording).mapResult { parseSessionAck(it.body).toLong() }
    }

    @JvmOverloads
    fun stopRecording(id: Long, cancel: Boolean = false): CompletableFuture<Long> {
        require(id in 1..0xffffffffL)
        return request(DeviceRequest.StopRecording(id.toUInt(), cancel)).mapResult {
            parseSessionAck(it.body).toLong().also { actual ->
                if (actual != id) throw RingException("protocol", "Recording session differs")
            }
        }
    }

    fun recordingStatus(): CompletableFuture<RecordingStatus> =
        request(DeviceRequest.RecordingStatus).mapResult {
            parseRecordingStatus(it.body).also { status ->
                recordingActive = status.state.toInt() in 1..3
            }
        }

    fun subscribeLive(enabled: Boolean): CompletableFuture<Boolean> {
        if (enabled) liveSubscribed = true
        return request(DeviceRequest.LiveSubscribe(enabled)).mapResult {
            parseLiveSubscription(it.body).also { actual ->
                liveSubscribed = actual
                if (actual != enabled)
                    throw RingException("protocol", "Subscription readback differs")
            }
        }
    }

    fun startImu(): CompletableFuture<ImuConfig> =
        request(DeviceRequest.StartImu).mapResult { parseImuConfig(it.body) }

    fun stopImu(): CompletableFuture<Void?> =
        request(DeviceRequest.StopImu).mapResult {
            parseSuccess(it.body)
            null
        }

    /** Destructive call requires an explicit true; no automatic file deletion. */
    fun clearAudio(confirmed: Boolean): CompletableFuture<Void?> =
        request(DeviceRequest.ClearAudio(confirmed)).mapResult {
            parseSuccess(it.body)
            null
        }

    @JvmOverloads
    fun downloadAudio(index: Long, maxBytes: Int = 16 * 1024 * 1024): CompletableFuture<ByteArray> {
        require(index in 0..0xffffffffL)
        require(maxBytes in 1..64 * 1024 * 1024)
        val result = CompletableFuture<ByteArray>()
        val owner = "audio-${System.nanoTime()}"
        launch(result) {
            try {
                session.acquireExclusive(owner)
            } catch (e: Throwable) {
                result.completeExceptionally(e)
                return@launch
            }
            var assembler: AudioAssembler? = null
            result.whenComplete { _, error ->
                enqueue {
                    assembler?.close()
                    if (error != null) fail(error)
                }
            }
            owned(DeviceRequest.AudioInfo(index.toUInt()), owner).whenComplete { packet, error ->
                enqueue {
                    if (result.isDone) return@enqueue
                    if (error != null) {
                        result.completeExceptionally(error)
                        return@enqueue
                    }
                    try {
                        val info = parseAudioInfo(packet.body)
                        if (info.fileIndex != index.toUInt())
                            throw RingException("protocol", "Audio file identity differs")
                        val data = AudioAssembler(info.fileIndex, info.dataSize, maxBytes.toUInt())
                        assembler = data
                        fun next() {
                            if (result.isDone) return
                            val offset = data.missingOffset()
                            if (offset == null) {
                                owned(DeviceRequest.AudioEnd(index.toUInt()), owner).whenComplete {
                                    ack,
                                    err ->
                                    if (err != null) result.completeExceptionally(err)
                                    else
                                        try {
                                            if (parseAudioEnd(ack.body) != index.toUInt())
                                                throw RingException(
                                                    "protocol",
                                                    "Audio completion identity differs",
                                                )
                                            val complete = data.finish()
                                            session.releaseExclusive(owner)
                                            result.complete(complete)
                                        } catch (e: Throwable) {
                                            result.completeExceptionally(e)
                                        }
                                }
                                return
                            }
                            owned(DeviceRequest.AudioRead(index.toUInt(), offset), owner)
                                .whenComplete { frame, err ->
                                    enqueue {
                                        if (result.isDone) return@enqueue
                                        if (err != null) result.completeExceptionally(err)
                                        else
                                            try {
                                                val value = parseAudioFrame(frame.body)
                                                if (
                                                    value.frameOffset != offset ||
                                                        value.data.isEmpty()
                                                )
                                                    throw RingException(
                                                        "protocol",
                                                        "Unexpected audio offset or empty frame",
                                                    )
                                                data.push(value)
                                                next()
                                            } catch (e: Throwable) {
                                                result.completeExceptionally(e)
                                            }
                                    }
                                }
                        }
                        next()
                    } catch (e: Throwable) {
                        result.completeExceptionally(e)
                    }
                }
            }
        }
        return result
    }

    /**
     * The returned transfer must verify the same device after reconnect before reporting
     * installation.
     */
    fun transferFirmware(
        bundle: ByteArray,
        trustedPublicKey: String,
        before: DeviceInfo,
        board: String,
        allowTest: Boolean,
    ): CompletableFuture<OtaTransfer> {
        val result = CompletableFuture<OtaTransfer>()
        val owner = "ota-${System.nanoTime()}"
        launch(result) {
            var transfer: OtaTransfer? = null
            var listener: AutoCloseable? = null
            try {
                if (recordingActive || liveSubscribed)
                    throw RingException("busy", "Stop recording and disable live audio before OTA")
                val caps =
                    capabilities
                        ?: throw RingException(
                            "incompatible",
                            "Device capabilities required for OTA",
                        )
                if (!supportsFeature(caps.features, "ota", 1u, 0u) || caps.boardTarget != board)
                    throw RingException("incompatible", "OTA contract or board differs")
                transfer = OtaTransfer(bundle, trustedPublicKey, before, board, allowTest)
                session.acquireExclusive(owner)
                val active = transfer
                listener = addPacketListener { packet ->
                    if (packet.command.toInt() in setOf(1105, 2001, 2201, 0x1ff)) {
                        try {
                            if (packet.command.toInt() == 0x1ff)
                                throw RingException("device", "OTA command rejected")
                            val step = active.handle(packet)
                            step.frame?.let {
                                transport.send(it).whenComplete { _, e ->
                                    if (e != null) enqueue { fail(e) }
                                }
                            }
                            if (step.transferComplete) {
                                listener?.close()
                                session.releaseExclusive(owner)
                                result.complete(active)
                            }
                        } catch (e: Throwable) {
                            result.completeExceptionally(e)
                        }
                    }
                }
                result.whenComplete { _, error ->
                    enqueue {
                        listener?.close()
                        if (error != null) {
                            active.cancel()
                            active.close()
                            fail(error)
                        }
                        try {
                            session.releaseExclusive(owner)
                        } catch (_: Throwable) {}
                    }
                }
                val ended: (Throwable) -> Unit = { result.completeExceptionally(it) }
                closeListeners.add(ended)
                result.whenComplete { _, _ -> closeListeners.remove(ended) }
                executor.schedule(
                    {
                        result.completeExceptionally(
                            RingException(
                                "timeout",
                                "OTA transfer timed out; installation is unconfirmed",
                            )
                        )
                    },
                    180,
                    TimeUnit.SECONDS,
                )
                transport.send(active.begin()).whenComplete { _, e ->
                    if (e != null) enqueue { fail(e) }
                }
            } catch (e: Throwable) {
                listener?.close()
                transfer?.close()
                try {
                    session.releaseExclusive(owner)
                } catch (_: Throwable) {}
                result.completeExceptionally(e)
            }
        }
        return result
    }

    private fun fail(error: Throwable) {
        if (!closed.compareAndSet(false, true)) return
        ready.completeExceptionally(error)
        dispatch(session.disconnect(generation))
        pending.values.forEach { it.completeExceptionally(error) }
        pending.clear()
        closeListeners.forEach { it(error) }
        closeListeners.clear()
        listeners.clear()
        try {
            transport.close()
        } finally {
            session.close()
            executor.shutdown()
        }
    }

    override fun close() {
        enqueue { fail(RingException("disconnected", "Connection closed")) }
    }
}
