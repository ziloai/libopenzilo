# OpenZilo Core

[English](README.md) · MPL-2.0

把 OpenZilo 的设备协议实现集中到一个 Rust 库，再通过 UniFFI 生成 Python、Kotlin
绑定。现有 Python SDK 和 Android Finder 共同使用它；第三方可以直接依赖 Android
SDK，开发自己的 App，无须依赖官方 App 的进程、账号或服务。

## 已实现的边界

- 核心：协议 v4 封包、CRC、分包；能力发现；系统信息、IMU 与设备事件解析；录音参数
  和状态；音频重组、OP2C 容器、直播帧校验；请求会话；签名固件校验与 OTA 状态。
- Android SDK：BluetoothGatt 连接、串行分片写入、异步请求、超时、取消、断连清理、
  有界事件 Flow，以及设备信息、能力、录音、下载、IMU、签名 OTA 接口。
- 平台层：Python 保留 Bleak/asyncio；Android 保留扫描和权限管理；Finder 保留系统
  RangingManager 与 ARCore 的相机引导。PCM 解码和播放使用平台或现有 Python 实现。

“蓝牙 6.0”不等于手机一定开放测距接口。这个库不改变系统支持情况，也不提供硬件
直接测角。AR 方向仍来自手机移动后的多点距离估算。没有公开 PPG 接口。

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
**arm64-v8a**；其他架构需要安装相应 Rust target、通过 `tools/build_android.py --abi`
构建，并调整 SDK 的 `abiFilters`。Python wheel 也需要按发布平台分别构建。

本地 Maven 仓库位于 `dist/maven/`。第三方 App 添加该仓库后依赖
`com.zilo:openzilo-sdk:0.1.0`。Maven 元数据包含 JNA、Kotlin 协程等依赖；仅复制 AAR
时需要自行添加这些依赖。详细调用示例见 [英文 README](README.md)、
[API 与生命周期](docs/api.md)，完整可编译示例见 `android/sample/`。

现有 Python SDK 在相邻的 `OpenZilo/` 中通过 uv 引用本库，公开 API 保持兼容。
正式发布前需要把本地路径依赖改成已发布的 wheel 版本。

OTA 只接受由独立可信公钥验证的签名包；测试固件需要明确选择允许。传输结束只代表
等待重启，重新连接同一设备并验证精确版本后才能确认安装成功。不会将生产私钥打包
到 SDK；删除录音仍需要显式确认。限制和实际验证范围见
[架构](docs/architecture.md)及[验证记录](docs/validation.md)。
