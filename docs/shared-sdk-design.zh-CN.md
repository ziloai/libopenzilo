# Rust 共享 SDK 设计提案

状态：根据 2026-10-27 检查推进的设计参考。当前已实现引擎/策略、下载与分块接收状态机、
共享校准/档案、音频导出、距离有效期和超时预算。两端组合调用这些对象，尚未统一为下文
概念性的单一 Effect 接口。可选原生解码器暂不可用，继续支持平台解码后端。
实际交付范围见 [API](api.md) 和 [验证记录](validation.md)。

目标：让 Rust 成为设备协议、能力策略、业务流程和可移植算法的统一实现。Python、
Android SDK 提供符合各自语言习惯的接口，并执行平台 I/O；第三方 App 获得与官方 App
相同的设备能力和校验行为。

## 1. 当前实现与缺口

当前 Rust 已有封包、CRC、分包、指令体、能力/数据解析、请求会话、音频重组、直播帧
校验、签名包验证与 OTA 传输状态。Python 已接入解析层，Android 还使用了共享会话、
音频重组和 OTA 对象。

仍可复用的业务实现：

| 来源 | 适合迁入 Rust 的内容 |
|---|---|
| Python `_client.py`；Android `ProtocolClient.kt` | 请求准入、逻辑操作状态、资源互斥、超时后的恢复规则、事件归属 |
| Python `_compatibility.py`；Android `discoverCapabilities` / `perform` | 能力协商、来源与未知状态、固件兼容档案、指令所需契约版本 |
| Python `_recording.py`；Android 录音方法 | 参数校验、读回一致性、启动 ACK 丢失后的未知状态、完成/保存确认 |
| Python `_audio.py`；Android `downloadAudio` | 普通/快速下载状态机、缺口和重传策略、结束确认、取消清理 |
| Python `_live_audio.py`；Rust `LiveValidator` | 订阅、START/DATA/END 顺序、会话与计数验证、限额、完整性结果 |
| Python `_ota.py`；Android `updateFirmware` | 完整升级生命周期、重连预算、同一设备验证、最终结果与进度 |
| Python `_imu_calibration.py`、`_imu_profiles.py` | 物理量换算、静态窗口、六面拟合、零偏、独立验证、档案格式与绑定校验 |
| Python `_codec.py`、`_pcm.py` | 音频格式识别、Speex/OP2C/Ogg/WAV 处理、PCM 流语义与可选解码器 |
| Finder 距离显示逻辑 | 距离观测规范化、可信度、时间戳有效性、断连和过期状态 |
| Python `_device.py` | 日志协议、时间同步协议、身份一致性规则；时钟读取和文件落盘留在宿主 |

这些文件包含平台适配代码，迁移时按职责拆分，不整文件翻译。Rust 中已有的对象应直接
复用或扩展，例如让 Python 的直播流程使用同一个 `LiveValidator`。

## 2. Rust 模块

先保持一个 Cargo package，按模块组织，避免立即拆成大量小 crate。

```text
src/
  model/          设备身份、能力、单位、时间戳、类型化错误
  protocol/       编解码、指令表、所有响应解析、协议版本
  engine/         请求路由、操作状态、资源协调、事件订阅、deadline
  device/         能力协商、兼容策略、身份验证、时间同步
  operations/
    recording/    配置、启动/停止、完成确认
    download/     普通/快速下载、重传、完整性、结束确认
    live_audio/   订阅和一段直播录音的生命周期
    ota/          预检、传输、等待重启、重连验证
    logs/         日志分块与完整性
  sensors/
    imu/          原始量程与物理单位、序号连续性
    calibration/  静态窗口、拟合、独立验证
    profiles/     档案 schema、设备/固件/数据来源匹配
  audio/          容器、PCM 块、可选编解码后端
  finding/        距离观测、质量、有效期与诊断
  ffi/            UniFFI 对象、记录、错误映射
```

传感器数据处理和健康分析算法可按产品需要增加 Rust 模块，通过统一接口供两端复用。

## 3. 统一业务引擎

采用由宿主驱动的状态机。首版不在核心内部引入隐藏的异步运行时、蓝牙线程或文件线程。

```text
用户操作 / BLE 通知 / 写完成 / 断连 / 时钟推进 / 重连结果
                         ↓
                   Rust DeviceEngine
                         ↓
发送帧 / 请求重连 / 取消计时 / 业务事件 / 操作完成或失败
```

拟议接口（概念接口，不是现有 API）：

```rust
start(operation, now) -> Vec<Effect>
on_notification(connection_generation, bytes, now) -> Vec<Effect>
on_write_completed(write_id, result, now) -> Vec<Effect>
on_disconnected(connection_generation, reason, now) -> Vec<Effect>
on_transport_ready(connection_generation, transport_info, now) -> Vec<Effect>
tick(now) -> Vec<Effect>
cancel(operation_id, now) -> Vec<Effect>
next_deadline() -> Option<MonotonicTime>
```

- 宿主对每个引擎串行投递事件；锁不能代替输入顺序约定。
- 平台报告传输层就绪和 MTU 等信息；Rust 自己发起设备信息/能力查询，避免在重连适配器里再实现一次协商。
- Rust 维护逻辑 operation/request/write/subscription ID；Python/Kotlin 维护对应的
  Future、Task、协程或 Flow 对象。
- 写操作区分“生成待发数据”“平台写完成”“设备确认”。发送进度不能代替设备确认。
- Rust 统一 deadline、重试预算和恢复决策；宿主读取单调时钟、安排唤醒并执行动作。
- 需要 Unix 时间的同步协议由宿主单独提供墙上时钟，不能拿单调时间代替。
- 平台必须执行清理动作并反馈结果；超时与取消不等于撤回已送达设备的指令。
- 不自动重试录音 START 等可能已执行的操作。协议没有线上的事务 ID，本地 ID 无法
  消除同一连接上旧响应与新请求的歧义；按操作返回“重连并查询状态”等恢复建议。

错误以稳定的代码和结构化参数输出，如设备拒绝、契约不匹配、数据缺口、结果未知、
需要重连和阶段超时；平台负责映射原有异常类型、用户提示及本地化，不用匹配错误字符串。

资源协调要表达真实状态：`Idle / Active / Finalizing / Unknown`，以及直播、IMU、下载和
升级的占用情况。录音 ACK 丢失后进入 Unknown；重新连接也不能直接推断设备空闲。
兼容矩阵由共享策略决定，OTA 等独占操作需要完成相应停止和确认。

## 4. 设备、能力和身份

- 统一 command → feature → required contract 表，移除 Python/Kotlin 各自硬编码的集合。
- 保留 `device / verified legacy profile / unknown` 来源；超时表示未知，不能等同于不支持。
- 固件兼容档案集中维护，严格匹配已验证设备和版本；未知版本不得依赖名称或前缀猜测。
- 缓存按连接 generation 和设备身份失效；OTA 后重新协商。
- 区分连接定位符与设备身份。MAC、BlueZ 地址、Android BluetoothDevice 只是宿主
  定位设备的方式，不能作为跨平台档案的唯一身份。
- 新档案优先绑定经读取核对的稳定设备身份；缺少可靠身份时标记为本平台有效，不自动
  将旧地址哈希档案转换为跨平台档案。固件、型号、量程及 raw/calibrated 来源仍需校验。
- 核心公开自身版本、构建身份和契约版本；绑定包另有版本。当前 Python 源码哈希不能
  单独代表已经链接的 Rust 核心。

## 5. 录音、下载、直播和 OTA

统一的是完整业务流程，接口返回类型化的领域事件：

- 录音：配置读回、session ID、采集中/落盘中/已保存/失败；停止 ACK 不代表已保存。
  明确区分可读的固件旧默认值与允许写入的配置。
- 下载：同一个引擎支持普通和快速方式；共享偏移、重复帧冲突、缺口、有限重传、大小
  上限与 END 确认。完成和取消策略明确，下载不隐含删除。
- 直播：共享 `LiveValidator` 及生命周期。Rust 验证序号、会话、计数和完整性；宿主负责
  异步分发。声明缓冲上限与溢出行为，不能静默丢帧后仍报告完整录音。
- OTA：统一预检、签名/目标兼容、停止冲突操作、传输、等待重启、重连预算、同一设备
  和精确版本核对。平台执行重连，Rust 判断升级是否已确认。
- 升级阶段、传输字节数、已验证运行版本分别呈现；保留独立可信公钥和测试固件选择，
  SDK 不包含生产私钥。删除录音继续要求应用和 CLI 显式确认。

Rust 不直接选保存目录或写用户文件。小文件可返回受限大小的内存结果；大文件提供
有界数据块和背压协议，由宿主选择文件或内存接收器，并反馈接收结果。

## 6. IMU、校准和音频

IMU 的批量换算、静止判定、饱和/漂移检查、六面拟合、陀螺零偏和独立验证全部适合迁移。
校准参数、单位、档案 schema 和 raw 数据来源检查共用；CLI 的操作提示、Android 的
向导页面、配置文件路径与原子文件写入留在平台。

区分 `RawImuBatch` 和 `CalibratedImuBatch`，防止重复校正。保留目前算法的能力边界：
对角加速度校准和陀螺零偏，不把它宣传为完整跨轴或陀螺比例校准。现有实验阈值在迁移时
保持行为一致并版本化；迁移本身不构成硬件精度验收。

音频先统一容器、帧有效性、PCM 单位/声道/采样位置和 WAV/Ogg 生成，再增加可选解码后端。
可在 Rust SDK 内包装成熟的 Opus/Speex 实现，无须为了语言一致性重写编解码算法。
FFmpeg/PyAV 调用仍是宿主可选后端。声卡、播放、系统音频路由留在平台。

可选解码能力需要 `library_features()` 查询；FFI 入口保持稳定，未编入后端时返回类型化
Unavailable。避免不同构建选项产生相互不匹配的 Python/Kotlin 绑定。

## 7. 距离查找

查找功能只做距离，不使用 AR、相机位姿、多点定位、方向箭头或空间位置求解。

Android 通过 RangingManager 获取标准 BLE CS 距离。Rust 未来统一观测格式、单位、
可信度映射、时间有效性及连接代次检查，输出 `Waiting / Measuring / LowQuality /
Stale / Unavailable / Disconnected` 状态。测距可信度沿用系统提供的语义，不把它转换为
未经标定的准确率。

距离输入包含来源、米制距离、测量时间及连接代次；宿主将系统时间转换到约定的单调
时间域。Rust 处理有限值、距离范围、旧连接回调和过期数据；平台负责权限、配对、系统
测距会话、显示和本地化。过期读数需要清空，不能沿用为当前距离。

首版保留原始有效距离，不引入未经验证的平滑或推算。Python 可回放同样的距离事件，
验证质量与过期策略；是否能获取实时 CS 仍取决于操作系统和硬件提供的接口。

## 8. 平台层最终保留什么

| Python SDK | Android SDK / App |
|---|---|
| Bleak 扫描、连接、GATT、MTU 和实际写队列 | BluetoothGatt、系统配对、权限与平台写队列 |
| asyncio Future/Task/迭代器、取消与事件桥接 | Kotlin suspend/Flow、Java Future 与生命周期桥接 |
| 文件路径、配置目录、文件接收器 | 应用存储、文件选择、系统后台执行策略 |
| CLI 参数、明确删除确认、终端进度和文本 | 页面、用户确认、进度展示与本地化 |
| 音频输出与可选外部解码后端 | AudioTrack/MediaCodec 等平台音频适配 |
| 可选桌面距离输入与回放文件读取 | RangingManager 测距会话及距离显示 |

BLE MTU 分片的最终执行留在适配层，因为平台写入/ACK 语义不同。Rust 控制协议帧和逻辑
发送顺序，宿主不得交错不同帧。FFI 按批量 IMU、音频块和事件批次交互，避免每个样本或
每个字节跨语言调用。

## 9. 迁移顺序和完成标准

1. **P0：业务引擎。** 统一错误、能力策略、资源状态、录音、下载、直播和 OTA 生命周期。
   Python 先调用现有 Rust 对象，再逐步换成统一引擎；保留公开 API 和异常兼容层。
2. **P1：IMU 与档案。** 移植已有算法和测试，增加跨语言一致性及旧档案受控迁移。
3. **P1：距离查找。** 统一距离观测、可信度和过期状态，Python 增加距离事件回放接口。
4. **P2：音频扩展。** 容器和流语义先统一，可选解码后端完成构建、许可及目标平台验证后接入。

每个模块以同一组去标识化/合成输入验证 Rust、Python、Android 得到相同结果、错误码和
状态推进；迁移后删除对应平台业务实现。继续保留公开 API 回归和设备实测。纯解析、
状态机、数值算法测试独立于蓝牙硬件运行；平台测试覆盖连接、写完成、取消、重连与资源释放。

首版统一引擎通过后，再决定是否将可选算法或编解码拆为独立 crate。所有阶段保持已交付
功能可构建，不用一次性重写替换整个 SDK。
