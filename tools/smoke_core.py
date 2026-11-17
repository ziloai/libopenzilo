#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""Run after installing a built wheel, without BLE or sibling checkouts."""
import libopenzilo as core
info = core.library_info()
assert info.api_contract == 1 and len(info.build_id) == 64
wire = core.encode_packet(0x101, b"")
assert core.decode_packet(wire).command == 0x101
bad = bytearray(core.encode_packet(0x605, b"corrupt"))
bad[-1] ^= 1
decoder = core.PacketDecoder()
batch = decoder.feed_batch(wire + bytes(bad) + wire)
assert batch.dropped_crc_packets == 1 and batch.fault is None
assert [packet.command for packet in batch.packets] == [0x101, 0x101]
assert core.command_contract(0x601).major == 2
rssi = core.prepare_request(core.DeviceRequest.CONNECTION_RSSI())
assert rssi.command == 0x107 and rssi.response == 0x108 and not rssi.body
assert core.parse_connection_rssi(b"\x00\x00\xbf") == -65
assert core.command_contract(0x107).feature == "connection_rssi"
opus = core.build_opus_recording([b"\xfc\xff\xfe"], 2)
assert core.opus_recording_info(opus).duration_ms == 20
assert core.export_ogg_opus(opus, 1).startswith(b"OggS")
assert core.export_ogg_speex([b"\0"], 16000, 1).startswith(b"OggS")
assert core.is_ogg_speex(core.export_ogg_speex([b"\0"], 16000, 1))
assert core.parse_packetized_speex_stream(b"\x03\x00abc\x00\x00", 540, 1026) == [b"abc"]
assert core.split_raw_speex_packets(b"a" * 40, 3, None) == [b"a" * 20, b"a" * 20]
assert core.parse_audio_record_result(b"\x00\x00\x00\x00\x00\x05") == 5
assert core.sensor_gesture_name(2) == "rotate_front"
collector = core.ImuStaticCollector(core.ImuConfig(sample_rate_hz=100, accel_range_g=16, gyro_range_dps=2000), 2000, 0, True)
assert collector.begin(0).target == 200
assert collector.next_deadline() == 12000
collector.cancel()
auto_audio = core.AutoAudioTransfer(1024, 1000, 10000)
assert auto_audio.begin(0).file_index is None
first = core.decode_packet(core.encode_packet(0x50a, b"\x00\x00\x00\x00\x00\x05"))
assert auto_audio.accept(first, 1).request.command == 0x503
auto_audio.cancel()
assert core.identity_imu_calibration(core.ImuConfig(sample_rate_hz=100, accel_range_g=16, gyro_range_dps=2000)).sensor_type == 0
engine = core.DeviceEngine(16)
generation = engine.open()
assert not engine.receive(bytes(bad), generation, 0)
assert engine.take_dropped_crc_packets() == 1
assert engine.take_dropped_crc_packets() == 0
engine.disconnect(generation)
print(f"libopenzilo {info.version} contract {info.api_contract}: wheel smoke passed")
