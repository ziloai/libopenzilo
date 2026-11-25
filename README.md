# libopenzilo

[中文](README.zh-CN.md) · MPL-2.0

**Unified OpenZilo interaction library.** A high-performance, pure-Rust core for the
OpenZilo protocol engine, device session state machines, audio decoding (Opus and
legacy Speex), IMU calibration with 6-DoF attitude fusion, and signed OTA. Native
Python and Kotlin bindings (UniFFI) give every platform identical data structures and
interaction semantics.

## Ecosystem and platform boundaries

| Repository | Responsibility |
| --- | --- |
| **libopenzilo** | Unified Rust interaction core, UniFFI bindings (`libopenzilo` / `com.zilo.libopenzilo`) and the cross-platform core tests |
| [OpenZilo](https://github.com/ziloai/OpenZilo) | Python BLE/asyncio SDK, CLI and Python examples built on `libopenzilo` |
| [OpenZilo-Connect](https://github.com/ziloai/OpenZilo-Connect) | Android companion app, BLE/GATT integration in `ring-core`, and the `connect-api` client AAR |

Hosts provide BLE scanning and GATT I/O, OS permissions, monotonic clocks, background
execution and file persistence. `libopenzilo` owns everything platform-independent:
wire frames, capability checks, request recovery, audio codecs, sensor calibration and
OTA verification. The crate has no BLE stack, UI or application runtime.

## Core capabilities

| Area | Capabilities |
| --- | --- |
| Protocol and transport rules | Protocol v4 framing and CRC; stream decoding that drops corrupt frames and keeps valid neighbors; NUS UUIDs and write limits, ring-name and address matching |
| Sessions and policy | `DeviceEngine` / `DeviceSession` request recovery, capability admission and legacy profiles, connection RSSI, recording state, `RingBindings` for several rings, shared error text |
| Audio | Pure-Rust Opus (`RingOpusDecoder`, `decode_opus_pcm`) and legacy Speex (`RingSpeexDecoder`, `decode_speex_pcm`) to PCM, dual-mic denoising, OP2C/Ogg/WAV containers, stored-file download and automatic reception, live validation; no ffmpeg or C codec |
| IMU | Raw-to-physical conversion, static-window collection, six-face and gyro-bias calibration with validation, 6-DoF quaternion attitude and posture classification, portable and scoped calibration profiles |
| Firmware | Signed bundle verification (ECDSA P-256), `OtaTransfer`, board identity and running-version checks |
| Distance | Validity, quality and expiry tracking for host-supplied measurements; no OS ranging and no public PPG interface |

## Pure-Rust audio

`RingOpusDecoder` and `decode_opus_pcm()` produce 16 kHz PCM16LE and preserve mono or
interleaved L/R stereo. `RingSpeexDecoder` and `decode_speex_pcm()` decode Ogg Speex,
length-prefixed or fixed-size raw packets to the requested rate, channels and depth.
For synchronized stereo with aligned speech, use one `DualMicDenoiser` per stream to
convert complete 20 ms frames to mono; it attenuates energy that differs between the
mics and does not align channels or remove noise common to both.

Use `LiveValidator` for complete live recordings. For preview playback only, opt in
with `LivePreviewValidator`: each accepted packet returns events in order, with a
`Gap` before the DATA or END that revealed missing 20 ms frames. Preview END
reconciles device drops, not completeness; the application chooses whether to conceal
the loss or insert silence.

Opus uses pure Rust [opus1](LICENSE-opus1) and Speex uses pure Rust `oxideav-speex`
([MIT](LICENSE-oxideav)); include these notices when distributing binaries.

## Multiple rings

`RingBindings` is optional. Bind a host-assigned ID to the CPU ID (or serial number),
persist `list()`, and `restore()` on restart. After probing a BLE link, `attach()`
checks the identity and returns a separate engine and generation; route callbacks to
that engine. Call `detach(id, attachment)` when the transport closes and detach before
`unbind()`. BLE, persistence and reconnect scheduling stay with the host.

## Build, bindings and validation

Building the core requires Rust 1.95 or newer and a system linker. Building Python
wheels also needs Python 3.10+ and maturin. The core has no native C library
dependencies and does not require a JDK, Android SDK or NDK.

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
maturin build --release --locked --out dist
```

The Rust crate, Python distribution and Python import are all named `libopenzilo`.

```python
import libopenzilo as core
wire = core.encode_packet(0x0101, b"")
assert core.decode_packet(wire).command == 0x0101
print(core.library_info().features)
```

Generate Kotlin bindings from the host library (Linux example):

```sh
cargo build --locked --features cli
cargo run --locked --features cli --bin uniffi-bindgen -- generate --library target/debug/liblibopenzilo.so --language kotlin --config uniffi.toml --out-dir build/bindings/kotlin --no-format
```

macOS uses `.dylib`; Windows uses `libopenzilo.dll`. Generated Kotlin records use
`com.zilo.libopenzilo`.

Python releases include wheels for glibc Linux (x86_64 and ARM64), macOS (Intel and
Apple Silicon), and Windows (x86_64). On platforms without a compatible wheel, pip
can build from the source distribution. This requires Rust 1.95+ with Cargo, a
working linker for the target, and access to the build dependencies. To request a
source build even when a wheel is available:

```sh
python -m pip install --no-binary=libopenzilo libopenzilo
```

CI installs and smoke-tests each wheel on its native runner and separately builds
and installs from the source archive. Publish validated core distributions before
dependent SDKs.

See [native API](docs/api.md), [architecture](docs/architecture.md) and
[validation](docs/validation.md). `library_info()` reports the linked library's
version, API contract and available features. Hardware behavior requires validation
on the target device.

## Community

- WeChat developer group

  <img src="docs/assets/wechat-group.png" alt="OpenZilo WeChat developer group QR code" width="160">

- [Discord](https://discord.gg/pTJZ3RMHA5)
- [GitHub Discussions](https://github.com/ziloai/OpenZilo/discussions)
