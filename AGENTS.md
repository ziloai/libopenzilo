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
- `android/sample/`: minimal consumer example, not the official application.
- Python BLE/asyncio adapters and CLI remain in the separate OpenZilo project.
  Finder UI, pairing and Android Ranging acquisition remain in the application.

Use `apply_patch` for source edits. Keep Chinese and English documentation aligned.
Never commit device identifiers, recordings, firmware, credentials, private keys
or generated native/build artifacts. Keep synthetic fixtures clearly identified.

Run `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings` and
`cargo test --locked`. Build/install the wheel after binding changes. For Android,
run `:sdk:testDebugUnitTest :sdk:assembleRelease :sample:assembleDebug` with the
project wrapper and installed JDK/SDK/NDK. Verify the published Maven consumer.
Hardware claims require separate device validation; tests are not hardware proof.
