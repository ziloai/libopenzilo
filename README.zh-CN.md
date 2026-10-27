# libopenzilo

[English](README.md) · MPL-2.0

把 OpenZilo 的设备协议实现集中到一个 Rust 库，再通过 UniFFI 生成 Python、Kotlin
绑定。开发者可以使用 Python 原生包或 Android SDK，连接兼容设备并处理数据。

## 仓库与产物

| 部分 | 名称 / 位置 | 职责 |
| --- | --- | --- |
| Rust 库与 Python 原生包 | `libopenzilo`，`src/` | 协议、状态机、校验及可移植算法 |
| Kotlin 原生绑定 | `com.zilo.libopenzilo` | 调用同一 Rust 实现，由 UniFFI 生成 |
| Android SDK | `com.zilo:openzilo-sdk`，`android/sdk/` | GATT、系统时钟、Future/Flow、连接与资源管理 |
| 示例应用 | `android/sample/` | 演示接入，不包含正式 App 的业务逻辑 |

Android SDK 与 Rust 库分别构建和打包；Android 类不会编入 Python wheel 或桌面原生库。
Python 的 Bleak/asyncio SDK 与 CLI 保留在 `OpenZilo` 仓库；界面、配对及系统测距采集
由接入应用负责。本地检出目录目前仍叫 `OpenZiloCore/`，仓库与包名称均为 `libopenzilo`。

## 已实现的边界

- 核心：协议 v4、能力准入、录音状态和请求恢复；普通/快速下载及分块接收；直播完整性
  校验；IMU 换算、校准及可移植档案；OP2C/Ogg/WAV 导出；距离有效期；签名校验与 OTA 状态。
- Android SDK：BluetoothGatt 连接、串行分片写入、异步请求、超时、取消、断连清理、
  有界事件 Flow，以及设备信息、能力、录音、下载、IMU、签名 OTA 接口。
- 平台层：Python 使用 Bleak/asyncio；Android 执行扫描、权限管理及系统测距采集。
  PCM 解码和播放使用平台或现有 Python 实现。

“蓝牙 6.0”不等于手机一定开放测距接口。这个库不改变系统支持情况，也不提供硬件
直接测角。查找功能只提供距离，不估算方向或空间位置。没有公开 PPG 接口。

## 构建和接入

工具应安装到常规位置：JDK 由系统包管理器安装；Android SDK 使用 `~/Android/Sdk`，
NDK 安装为其中的 `ndk/28.2.13676358`；Gradle 由项目 Wrapper 管理并缓存到
`~/.gradle`；maturin 使用 `uv tool install maturin` 安装。无需在临时目录解压工具链。

需要 Rust stable、Python 3.10+、JDK 21、Android SDK 36 / Build Tools 36.0.0、
NDK r28c 或兼容版本。Gradle 8.13、AGP 8.13.0、Kotlin 2.2.21。

```sh
cargo test --locked
maturin build --release --out dist
rustup target add aarch64-linux-android
export JAVA_HOME=/usr/lib/jvm/java-21-openjdk # Linux / Arch example
export ANDROID_HOME="$HOME/Android/Sdk"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/28.2.13676358"
cd android
./gradlew :sdk:testDebugUnitTest :sdk:assembleRelease :sample:assembleDebug
./gradlew :sdk:publishReleasePublicationToMavenRepository
```

Android 构建会自动编译 Rust 并生成 Kotlin 绑定。默认 AAR 和示例 APK 包含
**arm64-v8a**；安装相应 Rust target 后，使用
`-PopenziloAbis=arm64-v8a,x86_64,armeabi-v7a` 同时打包多个架构，`x86_64` 可用于模拟器。
CI 配置同时构建多架构 AAR，以及 Linux x86_64、Windows x86_64、macOS arm64 wheel；
远端任务通过后才能确认对应平台支持。

本地 Maven 仓库位于 `dist/maven/`。第三方 App 添加该仓库后依赖
`com.zilo:openzilo-sdk:0.1.0`。Maven 元数据包含 JNA、Kotlin 协程等依赖；仅复制 AAR
时需要自行添加这些依赖。详细调用示例见 [英文 README](README.md)、
[API 与生命周期](docs/api.md)，完整可编译示例见 `android/sample/`。

现有 Python SDK 在本地通过 uv 引用相邻的 `OpenZiloCore/`，发行包声明
`libopenzilo>=0.1.0,<0.2`，安装时不要求相邻源码。应先发布并验证核心 wheel，再分发
依赖它的 SDK。已有公开 API 保持兼容，新增分块下载和距离回放接口。

`library_info()` 返回已链接核心的版本、API 契约和可用功能。本版不包含原生 PCM 解码器，
`decode_opus_pcm()` 返回类型化的 `Unavailable`；Python 继续使用可选 FFmpeg/PyAV 后端。
Ogg 导出沿用 OP2C 的零预跳过约定，无法还原录音中未保存的编码器延迟元数据。

OTA 只接受由独立可信公钥验证的签名包；测试固件需要明确选择允许。传输结束只代表
等待重启，重新连接同一设备并验证精确版本后才能确认安装成功。不会将生产私钥打包
到 SDK；删除录音仍需要显式确认。限制和实际验证范围见
[架构](docs/architecture.md)及[验证记录](docs/validation.md)。
