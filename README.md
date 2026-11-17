# libopenzilo

[中文](README.zh-CN.md) · MPL-2.0

Portable OpenZilo protocol, device state machines, validation and data-processing
algorithms in Rust, with native Python and Kotlin bindings.

## Repository boundaries

| Repository | Responsibility |
| --- | --- |
| **libopenzilo** | Rust core, UniFFI configuration, native bindings and core tests |
| [OpenZilo](https://github.com/ziloai/OpenZilo) | Python BLE/asyncio SDK, CLI and Python examples |
| [openzilo-android](https://github.com/ziloai/openzilo-android) | Android BLE/GATT SDK, Future/Flow, AAR/Maven, tests and example app |

Python and Android are peer consumers of this library. Neither platform SDK is
maintained here. UI, permissions, background execution and file I/O belong to SDK
adapters or applications; this crate has no BLE stack or application runtime.

The core covers protocol v4/CRC, capability admission, request recovery,
connection RSSI queries, recording state, normal/fast downloads, automatic saved-recording
reception and sink backpressure, live validation,
IMU conversion/calibration, static-window collection and portable profiles,
packetized/raw Speex parsing,
OP2C/Ogg/WAV export, distance
validity, signed firmware verification and OTA transfer/running-version checks.
Distance utilities validate supplied measurements; they do not acquire OS ranging
measurements or estimate direction. No public PPG interface is provided.

Stream decoding drops complete frames with invalid body CRC while preserving valid
neighboring frames and pending requests. `DecodeBatch.dropped_crc_packets` and
`DeviceEngine.take_dropped_crc_packets()` expose diagnostics; single-packet decoding
still rejects CRC errors, and other malformed protocol data remains fatal to sessions.

## Build and validate

Rust stable is required. Python packaging additionally needs Python 3.10+ and
maturin; building this repository does not require a JDK, Android SDK or NDK.

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
maturin build --release --locked --out dist
```

The Rust crate, Python distribution and Python import are named `libopenzilo`.

```python
import libopenzilo as core
wire = core.encode_packet(0x0101, b"")
assert core.decode_packet(wire).command == 0x0101
```

Generate Kotlin bindings from the host library (Linux example):

```sh
cargo build --locked --features cli
cargo run --locked --features cli --bin uniffi-bindgen -- generate --library target/debug/liblibopenzilo.so --language kotlin --config uniffi.toml --out-dir build/bindings/kotlin --no-format
```

macOS uses `.dylib`; Windows uses `libopenzilo.dll`. Generated Kotlin records use
`com.zilo.libopenzilo`. Android compilation and integration are maintained in the
Android SDK repository, which pins a tested core revision.

Wheels are native to their build OS/architecture. CI builds Linux x86_64, macOS
arm64 and Windows x86_64 wheels, installs them and runs the native smoke check.
Publish validated core wheels before the dependent Python SDK.

See [native API](docs/api.md), [architecture](docs/architecture.md) and
[validation](docs/validation.md). `library_info()` reports the linked library's
version, API contract and available features. Native PCM decoding is unavailable
in this build; platform SDKs may use their own optional decoders.
