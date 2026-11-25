# Core validation

The portable library is validated independently of platform SDKs.

- Rust: 37 tests cover protocol/CRC, malformed input, request routing and recovery,
  recording uncertainty, download recovery and sink backpressure, stream integrity,
  IMU sequence/timestamp wrap, calibration, profile binding, audio containers,
  distance expiry, signed bundles and OTA identity/version verification.
- Formatting and Clippy checks run with warnings denied.
- Native Python wheels are installed and exercised through `tools/smoke_core.py`
  on glibc Linux x86_64 and ARM64, macOS Intel and Apple Silicon, and Windows
  x86_64. CI also installs from the source archive on Linux and runs the same
  smoke check. Validation and publishing use the same Python build workflow.
- Kotlin bindings are generated from the same host library in core CI.
- Platform integration is validated in the peer Python and Android SDK repositories.
  Android GATT, AAR/Maven, lifecycle and sample-app checks are not core tests.

Tests use synthetic fixtures. No usable firmware images, production private keys,
real device identifiers or captures are distributed. Parser/state-machine tests
and native loading checks do not establish hardware behavior, calibration accuracy
or a completed firmware installation. Those require separate device acceptance.

## Rust releases

Pushing a `vX.Y.Z` tag triggers `publish-crate.yml`. The workflow checks that the
tag matches `Cargo.toml`, runs the Rust tests, and publishes to crates.io with a
short-lived OIDC token, independently of the Python and Android release jobs.
The [crates.io trusted publisher](https://crates.io/docs/trusted-publishing) for
`libopenzilo` must use GitHub owner `ziloai`, repository `libopenzilo`, workflow
`publish-crate.yml`, and environment `crates-io`.
