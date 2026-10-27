# Android API and lifecycle

Use `OpenZiloDevice.connect(Context, BluetoothDevice)` after runtime permission
checks. It returns `CompletableFuture<OpenZiloDevice>` after NUS notification
subscription. `getSystemInfo()` returns Java-friendly `RingInfo`; `protocol`
exposes advanced Kotlin records and operations. `close()` releases the connection.
Cancel an unfinished connect future to close its GATT connection.

| ProtocolClient method | Result / behavior |
|---|---|
| `systemInfo()` | Core DeviceInfo, including firmware and device identity |
| `discoverCapabilities()` | Validated firmware/model identity and feature contracts |
| `audioCount()`, `audioInfo(index)` | Stored recording metadata |
| `downloadAudio(index, maxBytes)` | Verified bounded byte reassembly; leaves recordings intact |
| `recordingConfig()`, `setRecordingConfig(config)` | Volatile settings, verified readback |
| `startRecording()` | Nonzero session ID; capture completion is not confirmed yet |
| `stopRecording(id, cancel)` | Session ACK; poll status to confirm saved result |
| `recordingStatus()` | State, error and optional saved-file index |
| `subscribeLive(enabled)` | Subscribe before starting LIVE output; unsubscribe afterward |
| `startImu()`, `stopImu()` | IMU stream control; parse event bodies with core parsers |
| `clearAudio(confirmed = true)` | Explicit destructive confirmation required |
| `events()` | Bounded Flow of unsolicited core Packets |
| `request(DeviceRequest, timeout)` | Typed command specifications, including time/log requests |
| `transferFirmware(...)` | Signed transfer; returns OtaTransfer in AwaitingReboot phase |

Discover capabilities before enhanced commands. Unknown, missing or incompatible
contracts fail instead of silently enabling a feature. Configuration must satisfy
both the SDK profile and the device's advertised limits. A readback duration of
zero preserves the firmware's legacy unlimited default; outgoing settings require
a bounded duration of 20..60000 ms. A baseline system-info
query does not require capability discovery. Android SDK does not invent a legacy
feature profile for unrecognized firmware.

Each request has a finite timeout. Request futures support cancellation; typed
future cancellation propagates to the underlying request. No automatic retries
are made for state-changing commands. A cancelled or failed audio download closes
the transport to release its device-side extraction state. Audio and OTA acquire
exclusive connection ownership. A successful download does not delete the file.

Collect `events()` before starting an IMU report or live recording. The native
`parse_imu`, `parse_device_event` and `LiveValidator` functions validate their
bodies. Live audio packets are provisional until a successful END validates the
session, sequence, byte count and dropped-frame count. `build_opus_recording`
packages verified Opus packets into an OP2C file; it does not decode PCM.
`awaitResult()` bridges futures to cancellable Kotlin coroutines. Java callers
can use `whenComplete` and `thenCompose`, and should manage cancellation when
composing their own futures.

`OpenZiloDevice.updateFirmware(bundle, trustedPublicKey, allowTest)` performs
transfer, reconnection and exact running-version verification. Its result contains
the replacement device connection and RingInfo; close that device after use.
The old connection is closed and reconnection has a 60-second limit.
`transferFirmware` remains available for apps that supply their own reconnect UI.

OTA requires recording stopped, live subscription disabled, device capabilities,
an independently trusted public key and the correct board/model. Its transfer
result **does not confirm installation**. Close the old connection, reconnect the
same ring, obtain fresh system info, and call `OtaTransfer.verifyRunning(info)`.
Only phase `Verified` confirms the exact target version on the same identity.
Do not embed firmware signing private keys in an app. Close the returned transfer
when finished to release native memory. A timeout/disconnect leaves installation
unconfirmed and must be surfaced to the user.

Permissions, foreground/background execution and reconnection are application
policy. The sample uses foreground scanning only. SDK minimum Android is 8/API26;
standard CS in Finder needs Android16/API36 and actual OS feature availability.
Third-party apps may use their own RingTransport with ProtocolClient; transport
writes must serialize complete frames and report partial-write failure.
