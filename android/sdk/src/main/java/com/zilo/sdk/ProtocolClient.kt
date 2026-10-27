// SPDX-License-Identifier: MPL-2.0
package com.zilo.sdk

import com.zilo.libopenzilo.*
import java.util.concurrent.*
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.filter

/** A transport sends whole frames, serializes writes, and reports any partial-write failure. */
interface RingTransport : AutoCloseable {
    fun start(
        receiver: (ByteArray) -> Unit,
        disconnected: (Throwable) -> Unit,
    ): CompletableFuture<Void>

    fun send(frame: ByteArray): CompletableFuture<Void>
}

fun interface AudioSink {
    /** Return promptly; perform blocking file I/O on an application worker. */
    fun write(offset: Long, data: ByteArray): CompletableFuture<Void>
}

class RingException(val kind: String, message: String, val deviceCode: Int? = null, val code: ErrorCode? = null, val recovery: Recovery? = null) :
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
    private val session = DeviceEngine(256u)
    private val generation = session.open()
    private val pending = mutableMapOf<ULong, CompletableFuture<Packet>>()
    private val listeners = CopyOnWriteArrayList<(Packet) -> Unit>()
    private val closeListeners = CopyOnWriteArrayList<(Throwable) -> Unit>()
    private val closed = AtomicBoolean(false)
    private val imuStreamOwned = AtomicBoolean(false)
    private val liveStreamOwned = AtomicBoolean(false)
    val ready: CompletableFuture<Void> = CompletableFuture()
    @Volatile
    var capabilities: Capabilities? = null
        private set
    private var capabilityDiscovery: CompletableFuture<CapabilityProfile>? = null

    val deviceState: DeviceState get() = session.deviceState()

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
                        result.completeExceptionally(sdkError(e))
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
                        RingException(item.kind, item.message, item.deviceCode?.toInt(), item.code, item.recovery)
                    )
            is Delivery.Fault -> { fail(RingException(item.code.name.lowercase(), item.message)); return }
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

    /** One managed consumer; samples received before START ACK remain bounded. */
    fun imuStream(): Flow<ImuBatch> = managedImu { batch, _ -> batch }

    @JvmOverloads
    fun calibratedImuStream(profile: CalibrationProfile, localDeviceKey: String? = null): Flow<PhysicalImuBatch> =
        managedImu(profile, localDeviceKey) { batch, config ->
            applyImuCalibration(batch, config, profile.calibration, profile.accelValidated, profile.gyroValidated)
        }

    private fun <T> managedImu(
        profile: CalibrationProfile? = null,
        localDeviceKey: String? = null,
        convert: (ImuBatch, ImuConfig) -> T,
    ): Flow<T> = callbackFlow {
        if (!imuStreamOwned.compareAndSet(false, true)) throw RingException("busy", "IMU stream already has an owner")
        val continuity = ImuContinuity()
        val early = java.util.ArrayDeque<ImuBatch>()
        var config: ImuConfig? = null
        fun emitBatch(batch: ImuBatch) {
            continuity.accept(batch)
            if (trySend(convert(batch, config!!)).isFailure) close(RingException("overflow", "IMU consumer is too slow"))
        }
        val listener: (Packet) -> Unit = { packet ->
            if (packet.command.toInt() == 0x605) try {
                val batch = parseImu(packet.body)
                if (config == null) {
                    if (early.size >= 64) close(RingException("overflow", "IMU START acknowledgement delayed"))
                    else early.addLast(batch)
                } else emitBatch(batch)
            } catch (e: Throwable) { close(e) }
        }
        val ended: (Throwable) -> Unit = { close(it) }
        listeners.add(listener); closeListeners.add(ended)
        val starting = if (profile == null) startImu() else systemInfo().flatMapResult { info ->
            requireRawImuSource(session.profile().capabilities?.features ?: emptyList())
            startImu().mapResult { current ->
                checkProfileDevice(profile, info, localDeviceKey, current, session.profile().capabilities?.features ?: emptyList())
                current
            }
        }
        starting.whenComplete { value, e -> enqueue {
            if (e != null) close(e)
            else try { config = value; while (early.isNotEmpty()) emitBatch(early.removeFirst()) }
            catch (error: Throwable) { close(error) }
        } }
        awaitClose {
            listeners.remove(listener); closeListeners.remove(ended)
            starting.cancel(false)
            if (!closed.get()) stopImu().whenComplete { _, e -> if (e != null) enqueue { fail(e) } }
            continuity.close(); imuStreamOwned.set(false)
        }
    }

    /** A single provisional live recording; only END confirms completeness. */
    fun liveAudioStream(): Flow<LiveEvent> = callbackFlow {
        if (!liveStreamOwned.compareAndSet(false, true)) throw RingException("busy", "Live stream already has an owner")
        val validator = LiveValidator()
        val listener: (Packet) -> Unit = { packet ->
            if (packet.command.toInt() in 0x50f..0x511) try {
                val event = validator.accept(packet)
                if (trySend(event).isFailure) close(RingException("overflow", "Live consumer is too slow"))
                else if (event is LiveEvent.End) close()
            } catch (e: Throwable) { close(e) }
        }
        val ended: (Throwable) -> Unit = { close(it) }
        listeners.add(listener); closeListeners.add(ended)
        val starting = subscribeLive(true)
        starting.whenComplete { _, e -> if (e != null) close(e) }
        awaitClose {
            listeners.remove(listener); closeListeners.remove(ended)
            starting.cancel(false)
            if (!closed.get()) subscribeLive(false).whenComplete { _, e -> if (e != null) enqueue { fail(e) } }
            validator.close(); liveStreamOwned.set(false)
        }
    }

    fun deviceEvents(): Flow<DeviceEvent> = events().filter { it.command.toInt() in 0x701..0x704 }.map { parseDeviceEvent(it.command, it.body) }

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
                        session.checkAccess(spec.command, false)
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
                        result.completeExceptionally(sdkError(e))
                    }
                }
        }
        return result
    }

    fun systemInfo(): CompletableFuture<DeviceInfo> =
        request(DeviceRequest.SystemInfo).mapResult { parseSystemInfo(it.body) }

    fun discoverCapabilities(): CompletableFuture<Capabilities> =
        negotiateCapabilities().mapResult { profile ->
            profile.capabilities ?: throw RingException("incompatible", "Device capabilities remain unknown; reconnect before discovery retry")
        }

    /** Cache discovery for this connection, including timeout/unknown outcomes. */
    @Synchronized
    fun negotiateCapabilities(): CompletableFuture<CapabilityProfile> {
        capabilityDiscovery?.let { return it }
        val result = CompletableFuture<CapabilityProfile>()
        capabilityDiscovery = result
        val pipeline = systemInfo().flatMapResult { info ->
            val outcome = CompletableFuture<CapabilityProfile>()
            val discovery = request(DeviceRequest.Capabilities)
            discovery.whenComplete { _, error ->
                if (error == null) {
                    val profile = session.profile()
                    capabilities = profile.capabilities
                    outcome.complete(profile)
                } else {
                    val cause = if (error is CompletionException) error.cause ?: error else error
                    if (cause is RingException && (cause.kind == "timeout" || cause.deviceCode == 5)) {
                        val profile = session.useLegacyProfile(info, "Capability discovery unavailable; using only an exact verified profile")
                        capabilities = profile.capabilities
                        outcome.complete(profile)
                    } else outcome.completeExceptionally(cause)
                }
            }
            outcome.whenComplete { _, _ -> if (outcome.isCancelled) discovery.cancel(false) }
            outcome
        }
        pipeline.whenComplete { value, error ->
            if (error == null) result.complete(value) else result.completeExceptionally(error)
        }
        result.whenComplete { _, _ -> if (result.isCancelled) pipeline.cancel(false) }
        return result
    }

    fun audioCount(): CompletableFuture<Long> =
        request(DeviceRequest.AudioCount).mapResult { parseAudioCount(it.body).toLong() }

    fun audioInfo(index: Long): CompletableFuture<AudioFileInfo> {
        require(index in 0..0xffffffffL)
        val result = CompletableFuture<AudioFileInfo>()
        val owner = "audio-info-${System.nanoTime()}"
        launch(result) {
            try {
                session.acquireOperation(ExclusiveOperation.AUDIO_DOWNLOAD, owner)
            } catch (e: Throwable) {
                result.completeExceptionally(sdkError(e))
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
            parseRecordingStatus(it.body)
        }

    /** Stop ACK is not completion. Poll the same session with a finite total budget. */
    @JvmOverloads
    fun waitRecordingFinished(id: Long, totalTimeoutMs: Long = 75000, pollIntervalMs: Long = 200): CompletableFuture<RecordingStatus> {
        require(id in 1..0xffffffffL && totalTimeoutMs > 0 && pollIntervalMs > 0)
        val result = CompletableFuture<RecordingStatus>()
        launch(result) {
            val budget = DeadlineBudget(now(), totalTimeoutMs.toULong(), totalTimeoutMs.toULong())
            var query: CompletableFuture<Packet>? = null
            var timer: ScheduledFuture<*>? = null
            fun poll() {
                if (result.isDone) return
                try {
                    val remaining = budget.remaining(now()).toLong()
                    val current = request(DeviceRequest.RecordingStatus, minOf(timeoutMs, remaining))
                    query = current
                    current.whenComplete { packet, error -> enqueue {
                        if (!result.isDone) try {
                            if (error != null) result.completeExceptionally(error)
                            else {
                                val status = parseRecordingStatus(packet.body)
                                when (recordingCompletion(status, id.toUInt())) {
                                    RecordingCompletion.PENDING -> timer = executor.schedule({ poll() }, minOf(pollIntervalMs, budget.remaining(now()).toLong()), TimeUnit.MILLISECONDS)
                                    else -> result.complete(status)
                                }
                            }
                        } catch (e: Throwable) { result.completeExceptionally(sdkError(e)) }
                    } }
                } catch (e: Throwable) { result.completeExceptionally(sdkError(e)) }
            }
            result.whenComplete { _, _ -> enqueue { timer?.cancel(false); query?.cancel(false); budget.close() } }
            poll()
        }
        return result
    }

    @JvmOverloads
    fun stopAndWaitRecording(id: Long, cancel: Boolean = false): CompletableFuture<RecordingStatus> =
        stopRecording(id, cancel).flatMapResult { waitRecordingFinished(id) }

    fun subscribeLive(enabled: Boolean): CompletableFuture<Boolean> {
        return request(DeviceRequest.LiveSubscribe(enabled)).mapResult {
            parseLiveSubscription(it.body).also { actual ->
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
    fun downloadAudio(index: Long, maxBytes: Int = 16 * 1024 * 1024, quick: Boolean = false): CompletableFuture<ByteArray> {
        require(index in 0..0xffffffffL)
        require(maxBytes in 1..64 * 1024 * 1024)
        val result = CompletableFuture<ByteArray>()
        val owner = "download-${System.nanoTime()}"
        ready.whenComplete { _, readyError ->
            if (readyError != null) result.completeExceptionally(readyError)
            else launch(result) {
                session.acquireOperation(ExclusiveOperation.AUDIO_DOWNLOAD, owner)
                val transfer = DownloadTransfer(index.toUInt(), maxBytes.toUInt(), quick, timeoutMs.toULong(), 600000u)
                var timer: ScheduledFuture<*>? = null
                var subscription: AutoCloseable? = null
                fun drive(step: DownloadStep) {
                    if (result.isDone) return
                    if (step.phase == DownloadPhase.COMPLETE) {
                        val data = transfer.finish()
                        session.releaseExclusive(owner)
                        result.complete(data)
                        return
                    }
                    step.request?.let { spec ->
                        val frame = session.prepareSend(spec.command, spec.body, owner)
                        transport.send(frame).whenComplete { _, e ->
                            if (e != null) enqueue { result.completeExceptionally(sdkError(e)) }
                        }
                    }
                }
                subscription = addPacketListener { packet ->
                    if (packet.command.toInt() in setOf(0x504, 0x505, 0x508, 0x1ff) && !result.isDone) {
                        try { drive(transfer.accept(packet, now())) }
                        catch (e: Throwable) { result.completeExceptionally(sdkError(e)) }
                    }
                }
                val ended: (Throwable) -> Unit = { result.completeExceptionally(it) }
                closeListeners.add(ended)
                result.whenComplete { _, error -> enqueue {
                    subscription?.close()
                    timer?.cancel(false)
                    closeListeners.remove(ended)
                    if (error != null) { transfer.cancel(); fail(error) }
                    transfer.close()
                } }
                timer = executor.scheduleAtFixedRate({
                    if (!result.isDone) try { drive(transfer.tick(now())) }
                    catch (e: Throwable) { result.completeExceptionally(sdkError(e)) }
                }, 100, 100, TimeUnit.MILLISECONDS)
                try { drive(transfer.begin(now())) }
                catch (e: Throwable) { result.completeExceptionally(sdkError(e)) }
            }
        }
        return result
    }

    /** Sink futures accept each chunk before the next read; callers retain partial output on error. */
    @JvmOverloads
    fun downloadAudioToSink(index: Long, sink: AudioSink, maxBytes: Long = 1024L * 1024 * 1024): CompletableFuture<AudioFileInfo> {
        require(index in 0..0xffffffffL && maxBytes in 1..0xffffffffL)
        val result = CompletableFuture<AudioFileInfo>()
        val owner = "sink-${System.nanoTime()}"
        ready.whenComplete { _, e ->
            if (e != null) result.completeExceptionally(e)
            else launch(result) {
                session.acquireOperation(ExclusiveOperation.AUDIO_DOWNLOAD, owner)
                val transfer = AudioSinkTransfer(index.toUInt(), maxBytes.toUInt(), 600000u)
                var pendingWrite: CompletableFuture<*>? = null
                fun drive(step: AudioSinkStep) {
                    if (result.isDone) return
                    if (step.complete) {
                        val info = transfer.info()!!
                        session.releaseExclusive(owner)
                        result.complete(info)
                        return
                    }
                    if (step.chunk != null) {
                        val chunk = step.chunk!!
                        val writing = sink.write(chunk.offset.toLong(), chunk.data)
                        pendingWrite = writing
                        writing.whenComplete { _, error -> enqueue {
                            if (!result.isDone) try {
                                if (error != null) result.completeExceptionally(error)
                                else drive(transfer.acknowledgeChunk(chunk.offset, true, now()))
                            } catch (error: Throwable) { result.completeExceptionally(sdkError(error)) }
                        } }
                    } else {
                        val spec = step.request!!
                        val request = perform(spec, timeoutMs, owner)
                        pendingWrite = request
                        request.whenComplete { packet, error -> enqueue {
                            if (!result.isDone) try {
                                if (error != null) result.completeExceptionally(error)
                                else drive(transfer.accept(packet, now()))
                            } catch (error: Throwable) { result.completeExceptionally(sdkError(error)) }
                        } }
                    }
                }
                val deadline = executor.schedule({ result.completeExceptionally(RingException("timeout", "Audio sink total timeout")) }, 600, TimeUnit.SECONDS)
                val ended: (Throwable) -> Unit = { result.completeExceptionally(it) }
                closeListeners.add(ended)
                result.whenComplete { _, error -> enqueue {
                    deadline.cancel(false); closeListeners.remove(ended)
                    if (error != null) { pendingWrite?.cancel(false); transfer.cancel(); fail(error) }
                    transfer.close()
                } }
                try { drive(transfer.begin(now())) }
                catch (error: Throwable) { result.completeExceptionally(sdkError(error)) }
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
                session.requireOtaIdle()
                val caps =
                    capabilities
                        ?: throw RingException(
                            "incompatible",
                            "Device capabilities required for OTA",
                        )
                session.checkAccess(1005u, false)
                if (caps.boardTarget != board)
                    throw RingException("incompatible", "OTA board differs")
                transfer = OtaTransfer(bundle, trustedPublicKey, before, board, allowTest)
                session.acquireOperation(ExclusiveOperation.FIRMWARE_UPDATE, owner)
                val active = transfer
                val budget = DeadlineBudget(now(), timeoutMs.toULong(), 180000u)
                listener = addPacketListener { packet ->
                    if (packet.command.toInt() in setOf(1105, 2001, 2201, 0x1ff)) {
                        try {
                            if (packet.command.toInt() == 0x1ff)
                                throw RingException("device", "OTA command rejected")
                            budget.progress(now())
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
                            result.completeExceptionally(sdkError(e))
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
                val watchdog = executor.scheduleAtFixedRate({
                    if (!result.isDone) try { budget.remaining(now()) }
                    catch (_: Throwable) { result.completeExceptionally(RingException("timeout", "OTA transfer timed out; installation is unconfirmed")) }
                }, 100, 100, TimeUnit.MILLISECONDS)
                result.whenComplete { _, _ -> watchdog.cancel(false); budget.close() }
                transport.send(active.begin()).whenComplete { _, e ->
                    if (e != null) enqueue { fail(e) }
                }
            } catch (e: Throwable) {
                listener?.close()
                transfer?.close()
                try {
                    session.releaseExclusive(owner)
                } catch (_: Throwable) {}
                result.completeExceptionally(sdkError(e))
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

internal fun sdkError(error: Throwable): Throwable = when(error) {
    is CoreException.Incompatible -> RingException("incompatible", error.message ?: "Incompatible device", code = ErrorCode.INCOMPATIBLE)
    is CoreException.OutcomeUnknown -> RingException("outcome_unknown", error.message ?: "Outcome unknown", code = ErrorCode.OUTCOME_UNKNOWN, recovery = Recovery.RECONNECT_AND_QUERY_STATUS)
    is CoreException.Busy -> RingException("busy", error.message ?: "Device busy", code = ErrorCode.BUSY)
    is CoreException.Timeout -> RingException("timeout", "Request timed out", code = ErrorCode.TIMEOUT)
    is CoreException.Device -> RingException("device", "Device rejected request", error.code.toInt(), ErrorCode.DEVICE_REJECTED)
    is CoreException.Disconnected -> RingException("disconnected", "Disconnected", code = ErrorCode.DISCONNECTED)
    else -> error
}
