# Core validation

The portable library is validated independently of platform SDKs.

- Rust: 37 tests cover protocol/CRC, malformed input, request routing and recovery,
  recording uncertainty, download recovery and sink backpressure, stream integrity,
  IMU sequence/timestamp wrap, calibration, profile binding, audio containers,
  distance expiry, signed bundles and OTA identity/version verification.
- Formatting and Clippy checks run with warnings denied.
- Native Python wheels are installed and exercised through `tools/smoke_core.py`.
- Kotlin bindings are generated from the same host library in core CI.
- Platform integration is validated in the peer Python and Android SDK repositories.
  Android GATT, AAR/Maven, lifecycle and sample-app checks are not core tests.

Tests use synthetic fixtures. No usable firmware images, production private keys,
real device identifiers or captures are distributed. Parser/state-machine tests
and native loading checks do not establish hardware behavior, calibration accuracy
or a completed firmware installation. Those require separate device acceptance.
