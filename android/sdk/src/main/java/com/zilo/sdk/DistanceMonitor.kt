// SPDX-License-Identifier: MPL-2.0
package com.zilo.sdk

import android.os.SystemClock
import com.zilo.libopenzilo.*

data class DistanceValue(
    val state: DistanceState,
    val meters: Double?,
    val quality: DistanceQuality?,
    val measuredAtMs: Long?,
)

/** Java-friendly bridge. OS distance acquisition and permissions belong to the app. */
class DistanceMonitor @JvmOverloads constructor(maxAgeMs: Long = 2000, maxMeters: Double = 1000.0) : AutoCloseable {
    private val tracker = DistanceTracker(maxAgeMs.toULong(), maxMeters)
    init { require(maxAgeMs > 0) }
    @Synchronized fun open(generation: Long, available: Boolean) {
        require(generation >= 0)
        tracker.open(generation.toULong(), available, SystemClock.elapsedRealtime().toULong())
    }
    @Synchronized fun update(meters: Double, quality: DistanceQuality, measuredAtMs: Long, generation: Long): DistanceValue {
        require(measuredAtMs >= 0 && generation >= 0)
        return value(tracker.accept(DistanceObservation(meters, quality, DistanceSource.ANDROID_BLE_CS, measuredAtMs.toULong(), generation.toULong()), SystemClock.elapsedRealtime().toULong()))
    }
    @Synchronized fun current(): DistanceValue = value(tracker.tick(SystemClock.elapsedRealtime().toULong()))
    private fun value(reading: DistanceReading): DistanceValue = DistanceValue(
        reading.state, reading.observation?.meters, reading.observation?.quality,
        reading.observation?.measuredAtMs?.toLong(),
    )
    @Synchronized fun disconnect(generation: Long) = tracker.disconnect(generation.toULong())
    @Synchronized override fun close() = tracker.close()
}
