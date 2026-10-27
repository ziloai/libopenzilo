# libopenzilo development boundaries

This repository contains the portable Rust library and separately packaged Android
adapter SDK. The local checkout may still be named `OpenZiloCore`.

- `src/`: platform-independent protocol, capability policy, operation state,
  validation, portable data processing and container/profile serialization.
  Accept transport data and caller-provided monotonic time. Do not add BLE stacks,
  OS permissions, filesystem paths, network access, UI, playback, accounts or
  application background-service policy to the Rust crate.
- `uniffi.toml`: generated native bindings. Crate/Python import: `libopenzilo`;
  Kotlin binding namespace: `com.zilo.libopenzilo`.
- `android/sdk/`: Android BLE, GATT writes, Future/Flow bridges, OS clocks,
  connection/resource cleanup and device-level SDK conveniences. Portable rules
  and validation delegate to Rust. Artifact: `com.zilo:openzilo-sdk`.
- `android/sample/`: minimal SDK consumer example.
- Python BLE/asyncio adapters and CLI remain in the separate OpenZilo project.
  UI, pairing and Android Ranging acquisition belong to consuming applications.

Use `apply_patch` for source edits. Keep Chinese and English documentation aligned.
Do not name internal test applications or present them as official products in
repository documentation, diagrams, examples or validation reports.
Never commit device identifiers, recordings, firmware, credentials, private keys
or generated native/build artifacts. Keep synthetic fixtures clearly identified.

Run `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings` and
`cargo test --locked`. Build/install the wheel after binding changes. For Android,
run `:sdk:testDebugUnitTest :sdk:assembleRelease :sample:assembleDebug` with the
project wrapper and installed JDK/SDK/NDK. Verify the published Maven consumer.
Hardware claims require separate device validation; tests are not hardware proof.
