# libopenzilo

[English](README.md) · MPL-2.0

**统一的 OpenZilo 交互库。** 以高性能纯 Rust 实现 OpenZilo 协议引擎、设备会话状态机、
音频解码（Opus 与旧版 Speex）、IMU 校准与 6 轴姿态融合，以及签名 OTA，并通过 UniFFI
提供 Python 和 Kotlin 原生绑定，使各平台的数据结构与交互语义完全一致。

## 生态与平台边界

| 仓库 | 职责 |
| --- | --- |
| **libopenzilo** | 统一的 Rust 交互内核、UniFFI 绑定（`libopenzilo` / `com.zilo.libopenzilo`）及跨平台核心测试 |
| [OpenZilo](https://github.com/ziloai/OpenZilo) | 基于 `libopenzilo` 的 Python BLE/asyncio SDK、CLI 和 Python 示例 |
| [OpenZilo-Connect](https://github.com/ziloai/OpenZilo-Connect) | Android 配套应用、`ring-core` 的 BLE/GATT 接入，以及 `connect-api` 客户端 AAR |

宿主负责 BLE 扫描与 GATT 读写、系统权限、单调时钟、后台运行和文件持久化。
`libopenzilo` 负责所有与平台无关的部分：协议帧、能力检查、请求恢复、音频编解码、
传感器校准和 OTA 验证。本库不包含 BLE 栈、界面或应用运行时。

## 核心能力

| 领域 | 能力 |
| --- | --- |
| 协议与传输规则 | 协议 v4 封包与 CRC；丢弃损坏帧并保留相邻正常帧的流式解码；NUS UUID 与写入上限、戒指名称与地址匹配 |
| 会话与策略 | `DeviceEngine` / `DeviceSession` 请求恢复、能力准入与旧版档案、连接 RSSI、录音状态、多戒指 `RingBindings`、统一错误文案 |
| 音频 | 纯 Rust Opus（`RingOpusDecoder`、`decode_opus_pcm`）与旧版 Speex（`RingSpeexDecoder`、`decode_speex_pcm`）转 PCM、双麦降噪、OP2C/Ogg/WAV 容器、存储文件下载与自动接收、实时校验；不依赖 ffmpeg 或 C 编解码库 |
| IMU | 原始值到物理量换算、静态窗口采集、六面与陀螺零偏校准及验证、6 轴四元数姿态与姿态分类、可移植与隔离作用域校准档案 |
| 固件 | 签名固件包验证（ECDSA P-256）、`OtaTransfer`、板型标识与运行版本核对 |
| 距离 | 对宿主提供的测量值做有效性、质量和过期判断；不采集系统测距数据，没有公开 PPG 接口 |

## 纯 Rust 音频

`RingOpusDecoder` 和 `decode_opus_pcm()` 输出 16 kHz PCM16LE，保留原始单声道或左右交错的
双声道。`RingSpeexDecoder` 和 `decode_speex_pcm()` 可将 Ogg Speex、带长度前缀或定长的裸包
解码为指定采样率、声道数和位深。对于语音已对齐的同步双麦，每条流使用一个
`DualMicDenoiser`，将完整的 20 ms 帧转换为单声道；它按双麦差异衰减噪声，不负责通道对齐，
也不能去除两路共有的噪声。

完整实时录音使用 `LiveValidator`。仅用于预览播放时，显式选择 `LivePreviewValidator`：
每个通过校验的包按顺序返回事件，发现缺失的 20 ms 帧时在对应的 DATA 或 END 前发出 `Gap`。
预览 END 只核对设备丢帧，不保证完整录音；由应用决定做丢包隐藏还是补静音。

Opus 使用纯 Rust 的 [opus1](LICENSE-opus1)，Speex 使用纯 Rust 的 `oxideav-speex`
（[MIT](LICENSE-oxideav)）；分发二进制时须保留这些许可声明。

## 多戒指

`RingBindings` 是可选接口。宿主给戒指分配绑定 ID，以 CPU ID（或序列号）建立绑定，保存
`list()` 的记录并在重启后 `restore()`。探测 BLE 链路后，`attach()` 核对身份并返回该链路
独立的引擎和会话代数，传输回调应路由到该引擎。链路关闭时调用 `detach(id, attachment)`，
解绑前先断开。BLE、持久化和自动重连仍由宿主负责。

## 构建、绑定与验证

构建核心需要 Rust 1.95 或更新版本及系统链接器；构建 Python wheel 还需要 Python 3.10+ 和
maturin。核心不依赖原生 C 库，也不需要 JDK、Android SDK 或 NDK。

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
print(core.library_info().features)
```

从宿主原生库生成 Kotlin 绑定（Linux 示例）：

```sh
cargo build --locked --features cli
cargo run --locked --features cli --bin uniffi-bindgen -- generate --library target/debug/liblibopenzilo.so --language kotlin --config uniffi.toml --out-dir build/bindings/kotlin --no-format
```

macOS 使用 `.dylib`，Windows 使用 `libopenzilo.dll`。Kotlin 类型位于 `com.zilo.libopenzilo`。

Python 发行包提供 glibc Linux（x86_64 和 ARM64）、macOS（Intel 和 Apple Silicon）以及
Windows（x86_64）的 wheel。没有兼容 wheel 时，pip 可以从源码包构建，需要 Rust 1.95+、
Cargo、目标平台可用的链接器，以及构建依赖的访问权限。若想在有 wheel 时也强制从源码构建：

```sh
python -m pip install --no-binary=libopenzilo libopenzilo
```

CI 会在各平台的原生 runner 上安装并检查对应的 wheel，另行验证从源码包构建和安装。
先发布通过验证的核心发行包，再分发依赖它的 SDK。

详见[原生 API](docs/api.md)、[架构](docs/architecture.md)和[验证记录](docs/validation.md)。
`library_info()` 返回已链接库的版本、API 契约及可用功能。涉及硬件的行为需在目标设备上验证。

## 社区

- 微信开发者群

  <img src="docs/assets/wechat-group.png" alt="OpenZilo 开发者微信群二维码" width="160">

- [Discord](https://discord.gg/pTJZ3RMHA5)
- [GitHub Discussions](https://github.com/ziloai/OpenZilo/discussions)
