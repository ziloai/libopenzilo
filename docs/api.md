# Native library API

`libopenzilo` exposes portable Rust operations through UniFFI. Python imports
`libopenzilo`; generated Kotlin records use `com.zilo.libopenzilo`.

| Area | Entry points |
| --- | --- |
| Library identity | `library_info`, `core_version` |
| Protocol | `encode_packet`, `decode_packet`, `PacketDecoder`, `prepare_request` |
| Device session | `DeviceEngine`, `DeviceSession`, `Delivery`, `RequestTicket` |
| Capabilities | `command_contract`, `check_command_contract`, `CapabilityProfile` |
| Connection RSSI | `DeviceRequest::ConnectionRssi`, `parse_connection_rssi`; requires `connection_rssi` v1 |
| Recording | Configuration/status parsers, `recording_completion`, engine activity states |
| Stored audio | `DownloadTransfer`, `AudioSinkTransfer`, `AudioAssembler`, `AutoAudioTransfer` |
| Live audio | `LiveValidator` and typed START/DATA/END events |
| IMU | Raw/physical batches, `ImuContinuity`, `identity_imu_calibration`, `ImuStaticCollector`, calibration and validation functions |
| Profiles | Profile parsing, serialization and device/source binding checks |
| Audio | OP2C, Ogg Opus/Speex, PCM WAV, ring Opus-to-PCM decoding and aligned dual-mic suppression |
| Legacy Speex | `parse_packetized_speex_stream`, `split_raw_speex_packets`, `speex_packet_size`, `normalize_decoded_speex_pcm`, `is_wav`, `is_ogg_speex` |
| Device events | `parse_audio_record_result` (050A saved file index), `parse_device_event`, `sensor_gesture_name` |
| Distance | `DistanceTracker`, observations, quality and expiry states |
| Firmware | `inspect_firmware_bundle`, `OtaTransfer`, running identity/version verification |
| Timing | `DeadlineBudget`; caller-provided monotonic milliseconds |

The host serializes engine calls, sends complete request frames, supplies incoming
notifications/write failures/disconnects and drives deadlines. Timeout/cancellation
does not retract a device command. An ambiguous response slot is quarantined until
reconnect. Structured failure codes and recovery actions are independent of UI text.

`PacketDecoder.feed_batch()` consumes and drops complete frames whose body CRC is
invalid, continuing through valid neighboring frames and preserving partial frames.
`DecodeBatch.dropped_crc_packets` counts drops in that call, including when a later
fatal fault occurs. `feed()` shares this recovery behavior but does not return the
count; `decode_packet()` remains strict. Unsupported versions, oversized bodies and
other protocol faults are not treated as recoverable CRC errors.

`DeviceSession` and `DeviceEngine` keep the connection and pending requests alive
across CRC drops. Their `take_dropped_crc_packets()` returns and clears the accumulated
count since the previous call; opening a new connection resets it. Hosts may use this
structured diagnostic for warnings without treating it as an event or connection
fault, and stale-generation notifications do not affect it.

Sink transfers request the next chunk only after host acceptance. Download success
requires END acknowledgement; live data is provisional until verified END. OTA
transfer completion awaits reboot and exact same-device running-version verification.

`AutoAudioTransfer` accepts either a 050A saved-recording notification (then delegates
an explicit download with END acknowledgement) or legacy 0505 pushed frames. Legacy
pushes infer size from the end frame and request metadata only to recover a gap or
stall. Both paths bound storage, check overlaps and file identity, and use caller time.
The host must subscribe before the first event and serialize competing audio operations.

For legacy Speex, pass `540` as `max_packet_size` and `Some(1026)` as
`framed_block_size` (`1026` in Python); `None` disables outer-frame fallback. Packet
parsers return `None` for nonmatching data, allowing callers to try the fixed-size
format with quality `3` and `bits_size=None` (20-byte packets). Invalid zero size
options are errors. `normalize_decoded_speex_pcm` only normalizes bytes supplied by
a platform decoder; it does not decode Speex. Format detectors check signatures,
not the integrity of a complete container.

`ImuStaticCollector` discards settling samples and collects a bounded, contiguous
raw window before calling static analysis. Durations are milliseconds (2000–120000,
settling 0–30000); the deadline is settling + duration + 10000. Call `begin` on a
fresh stream after the host drains stale samples. A failed or cancelled collector
cannot be reused. Identity calibration keeps the sensor type unknown (`0`).
Recording configuration duration `0` means unlimited, matching the Python SDK;
nonzero durations remain subject to fixed-profile and advertised-device limits.

These entry points do not scan or connect BLE, manage platform permissions, open
files, acquire ranging measurements, play audio or provide language-native async
SDK operations. Those belong to the two peer SDKs:

- [Python SDK and CLI](https://github.com/ziloai/OpenZilo)
- [Android SDK](https://github.com/ziloai/openzilo-android)

See [architecture](architecture.md) for platform boundaries. Android lifecycle,
GATT, Future/Flow, AAR/Maven and sample-app documentation is maintained in the
Android SDK repository.
