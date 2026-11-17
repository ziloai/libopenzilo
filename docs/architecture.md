# Shared implementation and platform boundaries

This repository maintains only the portable library and native bindings. The
[Python SDK](https://github.com/ziloai/OpenZilo) and
[Android SDK](https://github.com/ziloai/openzilo-android) are peer repositories.
Android SDK sources, Gradle builds, platform tests and examples live in the latter.

The portable Rust library has no BLE stack, UI, filesystem, network, account or
background-service policy. Its exported UniFFI records are transport-neutral.
`prepare_request` defines command bodies; a transport sends complete encoded
frames and feeds received bytes to `DeviceEngine` (or the lower-level decoder/session).

Python and Android retain async scheduling, BLE I/O, file sinks, optional audio
decoders and actual reconnect attempts. Both use the shared engine's request
routing, capability contracts, recording/subscription states and uncertainty
rules. Normal/fast download policy, sink backpressure, live validation, calibration,
profile schema, container export, deadline budgets and OTA transfer/identity
verification execute in Rust. High-level adapters compose these core objects;
the proposal's single operation/effect envelope is not required by the current API.
The two languages have their own async adapters; Python cannot use Android GATT,
and a phone does not need to run a Python interpreter.

`OpenZiloDevice` supplies GATT connections and device operations. Applications
obtain distance observations through platform APIs such as Android RangingManager.
Rust determines distance validity and expiry; Python can replay equivalent
observations. The distance API does not estimate direction or spatial position.

The current protocol has no wire transaction ID. A local request ID prevents
callbacks from completing the wrong local future, but cannot disambiguate a late
reply after a same-command retry on the same physical connection. Reconnect
after an uncertain state-changing request; query recording status instead of
automatically retrying START. DeviceSession generation IDs reject callbacks from
older connections. A partial frame-write error makes the connection unusable.

The default Android event channel has bounded buffers. Slow consumers fail
explicitly. `DeviceEngine` delivers events directly to the host without duplicate
native storage. Lower-level `DeviceSession.receive()` users must drain
`take_events()`; `receive_direct()` is available for hosts with their own queues.
Body-CRC-invalid frames are consumed and dropped while valid neighbors and pending
requests survive; hosts obtain warning counts with `take_dropped_crc_packets()`.
Other malformed frames produce a connection fault after preceding valid deliveries;
remaining requests are failed explicitly.
Callbacks must be brief and must not block the session thread.

Firmware bundles use the signed stored ZIP format and P-256 signature over
the original manifest bytes, with independently provided PEM trust anchors.
The signed metadata and payload hash are verified before any OTA request.
The current firmware format supplies security-version metadata, but this SDK
does not assert an unread device rollback floor or bypass bootloader checks.

## Design and optional extensions

The implementation above describes the current source. See the
[shared SDK design proposal](shared-sdk-design.md) and its
[Chinese version](shared-sdk-design.zh-CN.md). The modules are implemented as
composable state machines in one crate. Optional native codec backends and further
API consolidation remain extensions; hardware acceptance is separate from tests.
