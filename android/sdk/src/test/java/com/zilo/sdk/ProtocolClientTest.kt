// SPDX-License-Identifier: MPL-2.0
package com.zilo.sdk

import com.zilo.libopenzilo.*
import java.util.concurrent.*
import org.junit.Assert.*
import org.junit.Test
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.flow.first

class ProtocolClientTest {
    private class Fake : RingTransport {
        lateinit var receiver: (ByteArray) -> Unit
        lateinit var disconnected: (Throwable) -> Unit
        val frames = LinkedBlockingQueue<ByteArray>()
        var isClosed = false
        var onSend: ((Packet) -> Unit)? = null
        var writeFailure: Throwable? = null

        override fun start(
            receiver: (ByteArray) -> Unit,
            disconnected: (Throwable) -> Unit,
        ): CompletableFuture<Void> {
            this.receiver = receiver
            this.disconnected = disconnected
            return CompletableFuture.completedFuture(null)
        }

        override fun send(frame: ByteArray): CompletableFuture<Void> {
            frames.add(frame)
            val packet = decodePacket(frame)
            if(packet.command.toInt() in setOf(0x503,0x509)) check(packet.body.size == 6 && packet.body[0] == 0.toByte() && packet.body[1] == 0.toByte())
            onSend?.invoke(packet)
            return if (writeFailure == null) CompletableFuture.completedFuture(null)
            else CompletableFuture.failedFuture(writeFailure!!)
        }

        fun incoming(command: Int, body: ByteArray = byteArrayOf()) {
            val data = encodePacket(command.toUShort(), body)
            receiver(data.copyOfRange(0, 5))
            receiver(data.copyOfRange(5, data.size))
        }

        fun next(): Packet =
            decodePacket(frames.poll(2, TimeUnit.SECONDS) ?: throw AssertionError("No write"))

        override fun close() {
            isClosed = true
        }
    }

    @Test
    fun requestRoutesFragmentedReplyAndClosesTransport() {
        val wire = Fake()
        val client = ProtocolClient(wire)
        try {
            val response = client.request(DeviceRequest.SystemInfo)
            assertEquals(0x101, wire.next().command.toInt())
            // A full typed system response, including empty identity strings.
            wire.incoming(0x102, ByteArray(25))
            assertEquals(0x102, response.get(2, TimeUnit.SECONDS).command.toInt())
        } finally {
            client.close()
        }
    }

    @Test
    fun standardErrorKeepsDeviceCode() {
        val wire = Fake()
        val client = ProtocolClient(wire)
        try {
            val response = client.request(DeviceRequest.SystemInfo)
            wire.next()
            wire.incoming(0x1ff, byteArrayOf(1, 1, 0, 5))
            try {
                response.get(2, TimeUnit.SECONDS)
                fail("Expected error")
            } catch (e: ExecutionException) {
                assertEquals(5, (e.cause as RingException).deviceCode)
            }
        } finally {
            client.close()
        }
    }

    @Test
    fun timeoutAndDisconnectCompletePending() {
        val wire = Fake()
        val client = ProtocolClient(wire, 50)
        try {
            val response = client.request(DeviceRequest.SystemInfo)
            wire.next()
            try {
                response.get(2, TimeUnit.SECONDS)
                fail("Expected timeout")
            } catch (e: ExecutionException) {
                assertEquals("timeout", (e.cause as RingException).kind)
            }
            val retry = client.request(DeviceRequest.SystemInfo)
            try { retry.get(2, TimeUnit.SECONDS); fail("Late reply must not complete a retry") }
            catch (e: ExecutionException) { assertEquals("outcome_unknown", (e.cause as RingException).kind) }
            val second = client.request(DeviceRequest.AudioCount)
            wire.next()
            wire.disconnected(IllegalStateException("gone"))
            try {
                second.get(2, TimeUnit.SECONDS)
                fail("Expected disconnect")
            } catch (_: ExecutionException) {}
        } finally {
            client.close()
        }
    }

    @Test
    fun partialWriteFailureClosesConnection() {
        val wire = Fake()
        wire.writeFailure = IllegalStateException("partial write")
        val client = ProtocolClient(wire)
        val response = client.request(DeviceRequest.SystemInfo)
        wire.next()
        try {
            response.get(2, TimeUnit.SECONDS)
            fail("Expected failure")
        } catch (_: ExecutionException) {}
        val after = client.request(DeviceRequest.AudioCount)
        try {
            after.get(2, TimeUnit.SECONDS)
            fail("Connection must be unusable")
        } catch (_: ExecutionException) {}
        client.close()
    }

    @Test
    fun typedFutureCancellationReachesRequest() {
        val source = CompletableFuture<Int>()
        val typed = source.mapResult { it.toString() }
        typed.cancel(false)
        assertTrue(source.isCancelled)
        val source2 = CompletableFuture<Int>()
        val child = CompletableFuture<String>()
        val composed = source2.flatMapResult { child }
        source2.complete(1)
        composed.cancel(false)
        assertTrue(child.isCancelled)
    }

    @Test
    fun requestCancellationQuarantinesAmbiguousResponse() {
        val wire = Fake()
        val client = ProtocolClient(wire)
        try {
            val first = client.request(DeviceRequest.SystemInfo)
            wire.next()
            first.cancel(false)
            val second = client.request(DeviceRequest.SystemInfo)
            try { second.get(2, TimeUnit.SECONDS); fail("Must reconnect before same-command retry") }
            catch (e: ExecutionException) { assertEquals("outcome_unknown", (e.cause as RingException).kind) }
            assertTrue(wire.frames.isEmpty())
        } finally {
            client.close()
        }
    }

    @Test
    fun enhancedOperationRequiresCapabilities() {
        val wire = Fake()
        val client = ProtocolClient(wire)
        try {
            val response = client.startImu()
            try {
                response.get(2, TimeUnit.SECONDS)
                fail("Expected capability gate")
            } catch (e: ExecutionException) {
                assertEquals("incompatible", (e.cause as RingException).kind)
            }
            assertTrue(wire.frames.isEmpty())
        } finally {
            client.close()
        }
    }

    @Test
    fun audioDownloadUsesReservedRequestFieldsAndReleasesOwnershipBeforeCompletion() {
        val wire = Fake()
        val client = ProtocolClient(wire)
        fun u32(n: Int) = java.nio.ByteBuffer.allocate(4).putInt(n).array()
        wire.onSend = { packet ->
            when (packet.command.toInt()) {
                0x503 -> wire.incoming(0x504, byteArrayOf(0, 0) + u32(0) + u32(1) + u32(4))
                0x506 -> {
                    assertArrayEquals(
                        byteArrayOf(0, 0) + u32(0) + u32(0) + byteArrayOf(0, 0),
                        packet.body,
                    )
                    wire.incoming(
                        0x505,
                        byteArrayOf(0, 0) +
                            u32(0) +
                            u32(0) +
                            u32(4) +
                            byteArrayOf(1, 11, 22, 33, 44),
                    )
                }
                0x507 -> wire.incoming(0x508, byteArrayOf(0, 0) + u32(0))
                0x501 -> wire.incoming(0x502, byteArrayOf(0, 0) + u32(1))
            }
        }
        try {
            val next =
                client.downloadAudio(0).flatMapResult { data ->
                    assertArrayEquals(byteArrayOf(11, 22, 33, 44), data)
                    client.audioCount()
                }
            assertEquals(1L, next.get(3, TimeUnit.SECONDS))
        } finally {
            client.close()
        }
    }

    @Test
    fun metadataQueryEndsExtractionAndAllowsNextOperation() {
        val wire = Fake()
        val client = ProtocolClient(wire)
        val index = byteArrayOf(0, 0, 0, 0)
        wire.onSend = { packet ->
            when (packet.command.toInt()) {
                0x503 ->
                    wire.incoming(
                        0x504,
                        byteArrayOf(0, 0) + index + index + byteArrayOf(0, 0, 0, 4),
                    )
                0x507 -> wire.incoming(0x508, byteArrayOf(0, 0) + index)
                0x501 -> wire.incoming(0x502, byteArrayOf(0, 0, 0, 0, 0, 1))
            }
        }
        try {
            val next =
                client.audioInfo(0).flatMapResult { info ->
                    assertEquals(4, info.dataSize.toInt())
                    client.audioCount()
                }
            assertEquals(1L, next.get(3, TimeUnit.SECONDS))
        } finally {
            client.close()
        }
    }

    @Test
    fun operationsAfterCloseDoNotHang() {
        val wire = Fake()
        val client = ProtocolClient(wire)
        wire.disconnected(IllegalStateException("gone"))
        Thread.sleep(50)
        for (future in
            listOf(client.audioInfo(0), client.downloadAudio(0), client.audioCount())) try {
            future.get(2, TimeUnit.SECONDS)
            fail("Expected closed connection")
        } catch (_: ExecutionException) {}
        client.close()
    }

    @Test
    fun sinkDownloadDoesNotReadAheadOfApplicationWrite() {
        val wire = Fake()
        val client = ProtocolClient(wire)
        fun u32(n: Int) = java.nio.ByteBuffer.allocate(4).putInt(n).array()
        val accepted = CompletableFuture<Void>()
        val delivered = CountDownLatch(1)
        wire.onSend = { packet -> when (packet.command.toInt()) {
            0x503 -> wire.incoming(0x504, byteArrayOf(0, 0) + u32(0) + u32(0) + u32(4))
            0x506 -> wire.incoming(0x505, byteArrayOf(0, 0) + u32(0) + u32(0) + u32(4) + byteArrayOf(1, 1, 2, 3, 4))
            0x507 -> wire.incoming(0x508, byteArrayOf(0, 0) + u32(0))
        } }
        try {
            val result = client.downloadAudioToSink(0, AudioSink { offset, data ->
                assertEquals(0L, offset)
                assertArrayEquals(byteArrayOf(1, 2, 3, 4), data)
                delivered.countDown()
                accepted
            })
            assertTrue(delivered.await(2, TimeUnit.SECONDS))
            assertEquals(0x503, wire.next().command.toInt())
            assertEquals(0x506, wire.next().command.toInt())
            assertNull(wire.frames.poll(50, TimeUnit.MILLISECONDS))
            assertFalse(result.isDone)
            accepted.complete(null)
            assertEquals(4, result.get(2, TimeUnit.SECONDS).dataSize.toInt())
            assertEquals(0x507, wire.next().command.toInt())
        } finally { client.close() }
    }

    @Test
    fun managedImuStreamNegotiatesSharedFixtureAndStopsAfterFirstBatch() = runBlocking {
        val wire = Fake()
        val client = ProtocolClient(wire)
        val hex = javaClass.getResourceAsStream("/capabilities-v1.hex")!!.bufferedReader().readText().trim()
        val fixture = hex.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
        val caps = parseCapabilities(fixture)
        fun text(value: String): ByteArray {
            val bytes = value.toByteArray()
            return byteArrayOf((bytes.size shr 8).toByte(), bytes.size.toByte()) + bytes
        }
        wire.onSend = { packet -> when (packet.command.toInt()) {
            0x101 -> wire.incoming(0x102, byteArrayOf(0, 0) + text(caps.firmwareVersion) + ByteArray(15) + text("") + text("synthetic") + text(caps.model))
            0x105 -> wire.incoming(0x106, fixture)
            0x601 -> {
                wire.incoming(0x602, byteArrayOf(0, 0, 0, 100, 0, 16, 7, 0xd0.toByte()))
                val sample = java.nio.ByteBuffer.allocate(16).putInt(1).putShort(2048.toShort()).put(ByteArray(10)).array()
                wire.incoming(0x605, byteArrayOf(0, 0, 0, 0, 0, 0) + sample)
            }
            0x603 -> wire.incoming(0x604, byteArrayOf(0, 0))
        } }
        try {
            assertEquals(CapabilitySource.DEVICE, client.negotiateCapabilities().get(2, TimeUnit.SECONDS).source)
            wire.frames.clear()
            val batch = withTimeout(3000) { client.imuStream().first() }
            assertEquals(2048, batch.samples.single().accelX.toInt())
            assertEquals(0x601, wire.next().command.toInt())
            assertEquals(0x603, wire.next().command.toInt())
        } finally { client.close() }
    }
}
