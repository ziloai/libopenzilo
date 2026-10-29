# libopenzilo development boundaries

This repository contains the portable Rust library and native language bindings.
The local checkout may still be named `OpenZiloCore`.

- `src/`: platform-independent protocol, capability policy, operation state,
  validation, portable data processing and container/profile serialization.
  Accept transport data and caller-provided monotonic time. Do not add BLE stacks,
  OS permissions, filesystem paths, network access, UI, playback, accounts or
  application background-service policy to the Rust crate.
- `uniffi.toml`: generated native bindings. Crate/Python import: `libopenzilo`;
  Kotlin binding namespace: `com.zilo.libopenzilo`.
- Android GATT, Future/Flow, platform clocks, SDK conveniences, examples and
  platform integration tests belong to the separate `ziloai/openzilo-android`
  repository. Never add Android SDK/Gradle/application sources here.
- Python BLE/asyncio adapters, SDK and CLI remain in `ziloai/OpenZilo`.
  Treat Python and Android as external consumers of the same shared library.

Use `apply_patch` for source edits. Keep Chinese and English documentation aligned.
Do not name internal test applications or present them as official products in
repository documentation, diagrams, examples or validation reports.
Never commit device identifiers, recordings, firmware, credentials, private keys
or generated native/build artifacts. Keep synthetic fixtures clearly identified.

Run `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings` and
`cargo test --locked`. Build/install the wheel and generate Kotlin bindings after
binding changes. Platform SDK acceptance runs in the respective SDK repositories.
Hardware claims require separate device validation; tests are not hardware proof.
