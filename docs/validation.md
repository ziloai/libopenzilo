# Local validation — 2026-10-27

- Rust: 37 tests passed, plus formatting and Clippy with warnings denied. Covers protocol golden bytes/CRC, fragmentation, malformed
  lengths, bounded storage, stale generations, timeouts/cancellation, device errors,
  exclusive ownership, partial-write failures, IMU, audio reassembly, capabilities,
  recording configuration and live stream ordering. New cases cover valid responses
  before receive faults, late-reply quarantine, stale disconnect callbacks,
  recording uncertainty, bounded download recovery and sink backpressure/deadlines,
  IMU sequence/timestamp wrap, calibration math/profile bindings, distance expiry,
  Ogg granules and WAV alignment.
- Synthetic signed ZIP tests cover trusted/wrong keys, tampering, size limits,
  board/profile checks, OTA transfer/reboot phases and exact post-reboot identity.
  There are no usable firmware images or production private keys in the fixtures.
- Python: 210 tests run, 204 passed and 6 conditional skips. Public dataclasses and SDK
  exceptions remain compatible. Native protocol, parsers and signed-bundle
  verification are exercised by the existing audio, recording and OTA tests.
  New adapter checks include immediate disconnect after a completed response,
  chunk-sink backpressure/failure, portable profiles and distance replay.
- Kotlin: 12 transport/session tests passed against the real host native library,
  including application sink acceptance and managed IMU start/data/stop.
  Native library changes are declared as test inputs, so Rust changes rerun them.
- A three-ABI Android AAR (arm64-v8a, x86_64, armeabi-v7a), local Maven repository, standalone example and
  Linux x86_64 Python wheel were built. The example also builds by consuming the
  Maven publication with `-PusePublishedSdk=true`. The core wheel and source archive
  install in isolated environments. The dependent Python SDK wheel also installs
  with the core wheel without a sibling checkout. Documentation generation and
  checks pass for 85 HTML pages.

## Earlier hardware baseline

These observations predate this migration and were not rerun on its final sources.
On the previous handset/ring baseline: native loading, BLE connection,
  system information, capability identity, recording count/info/configuration,
  and IMU start/data/stop passed. A stored file downloaded through the published
  Maven SDK matched the announced size, and the recording count stayed unchanged.
  These checks do not record, delete or update.

Hardware testing exposed two compatibility details now covered by regression
checks: audio START requests include the reserved u16 field, and configuration
readback can contain the firmware's legacy unlimited duration of zero. That value
is preserved when reading; outgoing configuration remains bounded.

The full Android OTA reconnect orchestration was not accepted on hardware.
OTA state/signature tests use synthetic data.
No hardware operations or firmware flashing were performed for this migration.
Windows/macOS wheel jobs are configured in CI but have not run locally. Native
PCM decoding reports Unavailable; FFmpeg/PyAV remain platform backends.

Builds use system JDK 21, SDK/NDK under `~/Android/Sdk`, and ordinary Cargo/Gradle
homes. Device identifiers, captures and recordings are never committed here.
