# Native library API

`libopenzilo` exposes portable Rust operations through UniFFI. Python imports
`libopenzilo`; generated Kotlin records use `com.zilo.libopenzilo`.

| Area | Entry points |
| --- | --- |
| Library identity | `library_info`, `core_version` |
| Protocol | `encode_packet`, `decode_packet`, `PacketDecoder`, `prepare_request` |
| Device session | `DeviceEngine`, `DeviceSession`, `Delivery`, `RequestTicket` |
| Capabilities | `command_contract`, `check_command_contract`, `CapabilityProfile` |
| Recording | Configuration/status parsers, `recording_completion`, engine activity states |
| Stored audio | `DownloadTransfer`, `AudioSinkTransfer`, `AudioAssembler` |
| Live audio | `LiveValidator` and typed START/DATA/END events |
| IMU | Raw/physical batches, `ImuContinuity`, calibration and validation functions |
| Profiles | Profile parsing, serialization and device/source binding checks |
| Audio export | OP2C, Ogg Opus/Speex, PCM WAV; optional decoder availability |
| Distance | `DistanceTracker`, observations, quality and expiry states |
| Firmware | `inspect_firmware_bundle`, `OtaTransfer`, running identity/version verification |
| Timing | `DeadlineBudget`; caller-provided monotonic milliseconds |

The host serializes engine calls, sends complete request frames, supplies incoming
notifications/write failures/disconnects and drives deadlines. Timeout/cancellation
does not retract a device command. An ambiguous response slot is quarantined until
reconnect. Structured failure codes and recovery actions are independent of UI text.

Sink transfers request the next chunk only after host acceptance. Download success
requires END acknowledgement; live data is provisional until verified END. OTA
transfer completion awaits reboot and exact same-device running-version verification.

These entry points do not scan or connect BLE, manage platform permissions, open
files, acquire ranging measurements, play audio or provide language-native async
SDK operations. Those belong to the two peer SDKs:

- [Python SDK and CLI](https://github.com/ziloai/OpenZilo)
- [Android SDK](https://github.com/ziloai/openzilo-android)

See [architecture](architecture.md) for platform boundaries. Android lifecycle,
GATT, Future/Flow, AAR/Maven and sample-app documentation is maintained in the
Android SDK repository.
