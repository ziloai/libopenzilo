# libopenzilo

[English](README.md) · MPL-2.0

以 Rust 实现 OpenZilo 协议、设备状态机、校验与可移植数据处理，并通过 UniFFI
提供 Python 和 Kotlin 原生绑定。

## 仓库边界

| 仓库 | 职责 |
| --- | --- |
| **libopenzilo** | Rust 核心、UniFFI 配置、原生绑定和核心测试 |
| [OpenZilo](https://github.com/ziloai/OpenZilo) | Python BLE/asyncio SDK、CLI 和 Python 示例 |
| [openzilo-android](https://github.com/ziloai/openzilo-android) | Android BLE/GATT SDK、Future/Flow、AAR/Maven、测试及接入示例 |

Python 与 Android 是平级的核心使用方，两端 SDK 均在独立仓库维护。界面、权限、
后台策略和文件 I/O 由平台适配或应用负责，Rust 核心不包含 BLE 栈或应用运行时。

核心包含协议 v4/CRC、能力准入、请求恢复、连接 RSSI 查询、录音状态、普通/快速下载、
自动保存录音接收及分块背压、
直播校验、IMU 换算与校准、静态窗口采集、可移植档案、分包/定长 Speex 解析、
OP2C/Ogg/WAV 导出、距离有效期、固件签名
验证以及 OTA 传输和运行版本核对。距离接口处理调用方传入的观测，不采集系统测距
数据，也不估算方向。没有公开 PPG 接口。

流式解码丢弃正文 CRC 错误的完整帧，同时保留前后正常帧和待完成请求；
`DecodeBatch.dropped_crc_packets` 和 `DeviceEngine.take_dropped_crc_packets()`
提供丢包诊断。单包解码仍拒绝 CRC 错误，其他协议格式错误仍会终止会话。

## 构建和验证

需要 Rust stable；构建 Python 包还需要 Python 3.10+ 和 maturin。
构建本仓库不需要 JDK、Android SDK 或 NDK。

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
maturin build --release --locked --out dist
```

Rust crate、Python 发行包和 Python 导入名均为 `libopenzilo`。

```python
import libopenzilo as core
wire = core.encode_packet(0x0101, b"")
assert core.decode_packet(wire).command == 0x0101
```

从宿主原生库生成 Kotlin 绑定（Linux 示例）：

```sh
cargo build --locked --features cli
cargo run --locked --features cli --bin uniffi-bindgen -- generate --library target/debug/liblibopenzilo.so --language kotlin --config uniffi.toml --out-dir build/bindings/kotlin --no-format
```

macOS 使用 `.dylib`，Windows 使用 `libopenzilo.dll`。Kotlin 类型位于
`com.zilo.libopenzilo`。Android 编译和接入验证由独立 SDK 仓库执行，并固定所验证的
核心提交。

wheel 与构建平台和架构对应。CI 构建 Linux x86_64、macOS arm64、Windows x86_64
产物，安装后运行原生接口检查。先发布通过验证的核心 wheel，再分发依赖它的 Python SDK。

详见[原生 API](docs/api.md)、[架构](docs/architecture.md)和[验证记录](docs/validation.md)。
`library_info()` 返回已链接库的版本、API 契约及可用功能。本版原生 PCM 解码不可用，
平台 SDK 可接入可选解码后端。
