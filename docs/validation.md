# Local validation — 2026-10-27

- Rust: 18 tests passed. Covers protocol golden bytes/CRC, fragmentation, malformed
  lengths, bounded storage, stale generations, timeouts/cancellation, device errors,
  exclusive ownership, partial-write failures, IMU, audio reassembly, capabilities,
  recording configuration and live stream ordering.
- Synthetic signed ZIP tests cover trusted/wrong keys, tampering, size limits,
  board/profile checks, OTA transfer/reboot phases and exact post-reboot identity.
  There are no usable firmware images or production private keys in the fixtures.
- Python: 203 tests passed with 10 conditional skips. Public dataclasses and SDK
  exceptions remain compatible. Native protocol, parsers and signed-bundle
  verification are exercised by the existing audio, recording and OTA tests.
- Kotlin: 10 transport/session tests passed against the real host native library.
  Native library changes are declared as test inputs, so Rust changes rerun them.
- An arm64 Android AAR, local Maven repository, standalone example, Finder APK and
  Linux x86_64 Python wheel were built. The example also builds by consuming the
  Maven publication with `-PusePublishedSdk=true`.
- On the user's Xiaomi 17 Pro and running ring: native loading, BLE connection,
  system information, capability identity, recording count/info/configuration,
  and IMU start/data/stop passed. A stored file downloaded through the published
  Maven SDK matched the announced size, and the recording count stayed unchanged.
  These checks do not record, delete or update.

Hardware testing exposed two compatibility details now covered by regression
checks: audio START requests include the reserved u16 field, and configuration
readback can contain the firmware's legacy unlimited duration of zero. That value
is preserved when reading; outgoing configuration remains bounded.

The full Android OTA reconnect orchestration was not accepted on hardware in
this change. OTA state/signature tests use synthetic data. Finder now has a
distance-only scope; camera/AR features and dependencies have been removed.
No new firmware was flashed during the shared SDK hardware check.

The final build uses system JDK 21, SDK/NDK under `~/Android/Sdk`, ordinary Cargo
and Gradle homes, and official repositories. Direct access with proxies disabled
and refreshed Gradle dependencies passed. Temporary tool/build directories were
removed. Device identifiers, captures and recordings are never committed here.
