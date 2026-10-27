# Shared implementation and platform boundaries

The portable Rust library has no BLE stack, UI, filesystem, network, account or
background-service policy. Its exported UniFFI records are transport-neutral.
`prepare_request` defines command bodies; a transport sends complete encoded
frames and feeds received bytes to `PacketDecoder` or `DeviceSession`.

Python's existing asyncio client retains its queue/callback scheduling,
capability fallback policy, audio operation ownership, optional FFmpeg/AV PCM
conversions and reconnection behavior. Its wire framing, system/audio/IMU/live
parsers, capability parser, recording configuration and firmware bundle
verification now delegate to the Rust core. Android's `ProtocolClient` uses the
shared request lifecycle, typed parsers, audio assembler and OTA transfer state.
The two languages have their own async adapters; Python cannot use Android GATT,
and a phone does not need to run a Python interpreter.

The official Finder uses `OpenZiloDevice` for its GATT connection and system-info
query. Android's RangingManager supplies CS distance. Finder displays distance and
measurement quality, and clears stale readings. Finding is distance-only; camera
tracking, direction estimation and spatial positioning are outside the scope.

The current protocol has no wire transaction ID. A local request ID prevents
callbacks from completing the wrong local future, but cannot disambiguate a late
reply after a same-command retry on the same physical connection. Reconnect
after an uncertain state-changing request; query recording status instead of
automatically retrying START. DeviceSession generation IDs reject callbacks from
older connections. A partial frame-write error makes the connection unusable.

The default Android event channel has bounded buffers. Slow consumers fail
explicitly. Native event storage is drained by ProtocolClient after each receive.
Direct DeviceSession users must call `take_events()` after processing deliveries
(or consume the storage themselves) to avoid filling that bounded queue.
Callbacks must be brief and must not block the session thread.

Firmware bundles use the ring_sound stored ZIP format and P-256 signature over
the original manifest bytes, with independently provided PEM trust anchors.
The signed metadata and payload hash are verified before any OTA request.
The current firmware format supplies security-version metadata, but this SDK
does not assert an unread device rollback floor or bypass bootloader checks.

## Proposed next boundary

The implementation above describes the current release. The proposed expansion
moves portable operation policies and algorithms into the Rust SDK while keeping
platform I/O and language-native async objects in the bindings. See the
[shared SDK design proposal](shared-sdk-design.md) and its
[Chinese version](shared-sdk-design.zh-CN.md). The proposal is not implemented yet.
