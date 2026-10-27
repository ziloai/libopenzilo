#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""Run after installing a built wheel, without BLE or sibling checkouts."""
import libopenzilo as core
info = core.library_info()
assert info.api_contract == 1 and len(info.build_id) == 64
wire = core.encode_packet(0x101, b"")
assert core.decode_packet(wire).command == 0x101
assert core.command_contract(0x601).major == 2
opus = core.build_opus_recording([b"\xfc\xff\xfe"], 2)
assert core.opus_recording_info(opus).duration_ms == 20
assert core.export_ogg_opus(opus, 1).startswith(b"OggS")
assert core.export_ogg_speex([b"\0"], 16000, 1).startswith(b"OggS")
engine = core.DeviceEngine(16)
generation = engine.open()
engine.disconnect(generation)
print(f"libopenzilo {info.version} contract {info.api_contract}: wheel smoke passed")
