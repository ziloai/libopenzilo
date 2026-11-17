// SPDX-License-Identifier: MPL-2.0
use libopenzilo::*;
fn packet(command: u16, body: Vec<u8>) -> Packet {
    decode_packet(encode_packet(command, body).unwrap()).unwrap()
}
#[test]
fn wire_golden_and_crc() {
    assert_eq!(crc16(b"123456789".to_vec(), 65535), 0x29b1);
    assert_eq!(
        encode_packet(0x101, vec![]).unwrap(),
        [63, 0, 4, 1, 1, 0, 0, 0, 0, 0, 0]
    );
    let mut d = encode_packet(0x602, vec![0, 0, 0, 100]).unwrap();
    assert_eq!(decode_packet(d.clone()).unwrap().body, [0, 0, 0, 100]);
    d[12] ^= 1;
    assert!(decode_packet(d).is_err());
    assert!(encode_packet(1, vec![0; 5121]).is_err());
}
#[test]
fn fragmentation_noise_and_concatenation() {
    let raw = encode_packet(0x102, (0..200).collect()).unwrap();
    for split in 0..raw.len() {
        let d = PacketDecoder::new();
        let mut all = d.feed(raw[..split].to_vec()).unwrap();
        all.extend(d.feed(raw[split..].to_vec()).unwrap());
        assert_eq!(all.len(), 1);
        assert_eq!(d.buffer_len(), 0);
    }
    let d = PacketDecoder::new();
    let mut b = vec![9, 8, 7];
    b.extend(&raw);
    b.extend(&raw);
    assert_eq!(d.feed(b).unwrap().len(), 2);
}
#[test]
fn invalid_stream_is_bounded_and_recovers() {
    let d = PacketDecoder::new();
    assert!(d
        .feed(vec![63, 0, 4, 1, 1, 255, 255, 255, 255, 0, 0])
        .is_err());
    assert_eq!(d.buffer_len(), 0);
    assert_eq!(d.feed(encode_packet(1, vec![]).unwrap()).unwrap().len(), 1);
    d.feed(vec![0; 100000]).unwrap();
    assert_eq!(d.buffer_len(), 0);
}
#[test]
fn request_routing_stale_generation_timeout_cancel() {
    let s = DeviceSession::new(4).unwrap();
    let g = s.open().unwrap();
    let a = s.begin(0x101, 0x102, vec![], 0, 10, None).unwrap();
    assert!(s.begin(0x101, 0x102, vec![], 0, 10, None).is_err());
    assert!(s
        .receive(encode_packet(0x102, vec![]).unwrap(), g + 1, 1)
        .unwrap()
        .is_empty());
    assert!(
        matches!(s.receive(encode_packet(0x102,vec![]).unwrap(),g,1).unwrap()[0],Delivery::Response{request_id,..}if request_id==a.id)
    );
    let b = s.begin(1, 2, vec![], 1, 10, None).unwrap();
    assert!(matches!(s.tick(11)[0],Delivery::Failure{request_id,..}if request_id==b.id));
    assert!(matches!(
        s.begin(1, 2, vec![], 12, 10, None),
        Err(CoreError::OutcomeUnknown)
    ));
    s.disconnect(g);
    let g = s.open().unwrap();
    let c = s.begin(1, 2, vec![], 12, 10, None).unwrap();
    assert!(s.cancel(c.id).is_some());
    assert!(s.begin(1, 2, vec![], 12, 10, None).is_err());
    let d = s.begin(3, 4, vec![], 12, 10, None).unwrap();
    assert_eq!(s.disconnect(g).len(), 1);
    assert!(s.cancel(d.id).is_none());
    assert!(s.open().unwrap() > g);
}
#[test]
fn device_errors_backpressure_exclusive_and_partial_write() {
    let s = DeviceSession::new(1).unwrap();
    let g = s.open().unwrap();
    let t = s.begin(0x101, 0x102, vec![], 0, 50, None).unwrap();
    let out = s
        .receive(encode_packet(0x1ff, vec![1, 1, 0, 5]).unwrap(), g, 1)
        .unwrap();
    assert!(
        matches!(out[0],Delivery::Failure{request_id,device_code:Some(5),..}if request_id==t.id)
    );
    s.acquire_exclusive("ota".into()).unwrap();
    assert!(s.begin(1, 2, vec![], 0, 50, None).is_err());
    assert!(s.release_exclusive("other".into()).is_err());
    s.release_exclusive("ota".into()).unwrap();
    s.receive(encode_packet(7, vec![]).unwrap(), g, 2).unwrap();
    assert!(matches!(
        s.receive(encode_packet(8, vec![]).unwrap(), g, 2)
            .unwrap()
            .last(),
        Some(Delivery::Fault {
            code: ErrorCode::Overflow,
            ..
        })
    ));
    assert!(s.take_events().is_empty());
    s.interrupt_write(g);
    assert!(s.begin(1, 2, vec![], 0, 50, None).is_err());
}
#[test]
fn audio_reassembly_retransmission_and_identity() {
    let a = AudioAssembler::new(4, 4, 100).unwrap();
    assert!(a.finish().is_err());
    a.push(AudioFrame {
        file_index: 4,
        frame_offset: 2,
        frame_size: 2,
        is_end: true,
        data: vec![3, 4],
    })
    .unwrap();
    assert_eq!(a.missing_offset(), Some(0));
    let f = AudioFrame {
        file_index: 4,
        frame_offset: 0,
        frame_size: 2,
        is_end: false,
        data: vec![1, 2],
    };
    a.push(f.clone()).unwrap();
    a.push(f).unwrap();
    assert_eq!(a.finish().unwrap(), [1, 2, 3, 4]);
    assert!(a
        .push(AudioFrame {
            file_index: 4,
            frame_offset: 1,
            frame_size: 1,
            is_end: false,
            data: vec![9]
        })
        .is_err());
    assert!(AudioAssembler::new(0, 101, 100).is_err());
}
#[test]
fn opus_roundtrip_and_rejection() {
    let data = build_opus_recording(vec![vec![0x7c, 0]], 2).unwrap();
    assert_eq!(
        parse_opus_recording(data.clone()).unwrap().packets,
        vec![vec![0x7c, 0]]
    );
    assert!(build_opus_recording(vec![vec![0x78, 0]], 2).is_err());
    assert!(parse_opus_recording(data[..data.len() - 1].to_vec()).is_err());
}
#[test]
fn imu_legacy_batch_and_signed_samples() {
    let mut b = vec![0, 0];
    b.extend(42u32.to_be_bytes());
    let mut sample = 99u32.to_be_bytes().to_vec();
    for n in [-1i16, 2, -3, 4, -5, 6] {
        sample.extend(n.to_be_bytes());
    }
    b.extend(&sample);
    let one = parse_imu(b).unwrap();
    assert_eq!(one.samples[0].accel_x, -1);
    let mut b = vec![0, 0];
    b.extend(42u32.to_be_bytes());
    b.extend([0, 2, 0, 16]);
    b.extend(&sample);
    b.extend(sample);
    assert_eq!(parse_imu(b.clone()).unwrap().frame_count, 2);
    b.pop();
    assert!(parse_imu(b).is_err());
}
#[test]
fn command_golden_guards() {
    let rssi = prepare_request(DeviceRequest::ConnectionRssi).unwrap();
    assert_eq!(rssi.command, 0x107);
    assert_eq!(rssi.response, Some(0x108));
    assert!(rssi.body.is_empty());
    assert_eq!(rssi.feature.as_deref(), Some("connection_rssi"));
    let r = prepare_request(DeviceRequest::AudioRead {
        file_index: 5,
        offset: 99,
    })
    .unwrap();
    assert_eq!(r.body, [0, 0, 0, 0, 0, 5, 0, 0, 0, 99, 0, 0]);
    assert!(prepare_request(DeviceRequest::ClearAudio { confirmed: false }).is_err());
    assert!(prepare_request(DeviceRequest::StopRecording {
        session_id: 0,
        cancel: false
    })
    .is_err());
}
#[test]
fn live_sequence_and_verified_end() {
    let mut b = 1u32.to_be_bytes().to_vec();
    b.extend([0; 4]);
    b.extend(99u32.to_be_bytes());
    b.extend([0; 4]);
    b.extend([0x3e, 0x80, 2, 3]);
    let v = LiveValidator::new();
    v.accept(packet(0x50f, b.clone())).unwrap();
    let mut data = 1u32.to_be_bytes().to_vec();
    data.extend([0; 8]);
    data.extend([0, 1, 0x7c]);
    v.accept(packet(0x510, data.clone())).unwrap();
    let mut end = vec![0, 0];
    for n in [1u32, 0, 1, 1, 0] {
        end.extend(n.to_be_bytes());
    }
    v.accept(packet(0x511, end)).unwrap();
    assert!(v.accept(packet(0x510, data.clone())).is_err());
    let v = LiveValidator::new();
    v.accept(packet(0x50f, b)).unwrap();
    data[7] = 1;
    assert!(v.accept(packet(0x510, data)).is_err());
}
#[test]
fn connection_rssi_signed_range_and_strict_response() {
    for value in [-127i8, -65, -1, 0, 20] {
        assert_eq!(
            parse_connection_rssi(vec![0, 0, value as u8]).unwrap(),
            value
        );
    }
    assert!(matches!(
        parse_connection_rssi(vec![0, 7]),
        Err(CoreError::Device { code: 7 })
    ));
    for body in [
        vec![],
        vec![0],
        vec![0, 0],
        vec![0, 0, 128],
        vec![0, 0, 21],
        vec![0, 0, 0, 0],
        vec![0, 7, 0],
    ] {
        assert!(matches!(
            parse_connection_rssi(body),
            Err(CoreError::Invalid { .. })
        ));
    }
    let feature = FeatureVersion {
        name: "connection_rssi".into(),
        major: 1,
        minor: 0,
    };
    assert!(check_command_contract(0x107, vec![feature], CapabilitySource::Device, false).is_ok());
    assert!(check_command_contract(0x107, vec![], CapabilitySource::Device, false).is_err());
    assert!(check_command_contract(0x107, vec![], CapabilitySource::Unknown, false).is_err());
    assert!(feature_contracts()
        .iter()
        .any(|f| f.name == "connection_rssi"));
}
#[test]
fn legacy_0000_admits_only_ota_for_the_exact_identity() {
    let mut info = DeviceInfo {
        firmware_version: "V2.001.0002.0000-prod-bccb2ffc".into(),
        system_time: 0,
        audio_storage_total: 0,
        audio_storage_available: 0,
        battery_percent: 0,
        battery_charging: false,
        sn: String::new(),
        cpuid: String::new(),
        model: "Open Zilo".into(),
    };
    let profile = legacy_capability_profile(info.clone(), "synthetic legacy fixture".into());
    assert_eq!(profile.source, CapabilitySource::VerifiedLegacyProfile);
    let caps = profile.capabilities.unwrap();
    assert_eq!(caps.board_target, "ring_dvt2/nrf54l15/cpuapp");
    assert_eq!(caps.features.len(), 1);
    assert!(check_command_contract(1005, caps.features.clone(), profile.source, false).is_ok());
    assert!(check_command_contract(0x51a, caps.features, profile.source, false).is_err());
    assert!(caps.recording_limits.is_none());
    info.model = "Other ring".into();
    assert!(legacy_capability_profile(info.clone(), String::new())
        .capabilities
        .is_none());
    info.model = "Open Zilo".into();
    info.firmware_version = "V2.001.0002.0000-prod-deadbeef".into();
    assert!(legacy_capability_profile(info, String::new())
        .capabilities
        .is_none());
}
#[test]
fn capabilities_fixture() {
    let hex = include_str!("fixtures/capabilities-v1.hex");
    let h: String = hex
        .lines()
        .filter(|l| !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join("")
        .split_whitespace()
        .collect();
    let b = (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
        .collect();
    let mut b: Vec<u8> = b;
    b.extend([1, 12, 0, 2, 1, 0]);
    let c = parse_capabilities(b).unwrap();
    assert!(supports_feature(
        c.features.clone(),
        "connection_rssi".into(),
        1,
        0
    ));
    let legacy = legacy_capability_profile(
        DeviceInfo {
            firmware_version: "V2.001.0002.0001-prod-7fd1dfc9".into(),
            system_time: 0,
            audio_storage_total: 0,
            audio_storage_available: 0,
            battery_percent: 0,
            battery_charging: false,
            sn: String::new(),
            cpuid: String::new(),
            model: "Open Zilo".into(),
        },
        "synthetic legacy fixture".into(),
    );
    assert!(!legacy
        .capabilities
        .unwrap()
        .features
        .iter()
        .any(|f| f.name == "connection_rssi"));
    assert!(supports_feature(c.features, "imu".into(), 2, 0));
    assert_eq!(c.recording_limits.unwrap().sample_rate_hz, 16000);
}
#[test]
fn speex_packetized_raw_and_container_detection() {
    let packets = vec![vec![1, 2, 3], vec![7; 255]];
    let mut wire = Vec::new();
    for packet in &packets {
        wire.extend((packet.len() as u16).to_le_bytes());
        wire.extend(packet);
    }
    wire.extend([0, 0, 255]);
    assert_eq!(
        parse_packetized_speex_stream(wire.clone(), 540, None).unwrap(),
        Some(packets.clone())
    );
    assert!(parse_packetized_speex_stream(wire.clone(), 254, None)
        .unwrap()
        .is_none());
    *wire.last_mut().unwrap() = 1;
    assert!(parse_packetized_speex_stream(wire, 540, None)
        .unwrap()
        .is_none());
    for bad in [vec![], vec![0, 0], vec![3, 0, 1], vec![255, 255]] {
        assert!(parse_packetized_speex_stream(bad, 540, None)
            .unwrap()
            .is_none());
    }
    let mut block = vec![3, 0, 1, 2, 3];
    block.resize(1026, 0);
    let mut framed = block.clone();
    framed.extend(&block);
    assert!(parse_packetized_speex_stream(framed.clone(), 540, None)
        .unwrap()
        .is_none());
    assert_eq!(
        parse_packetized_speex_stream(framed.clone(), 540, Some(1026)).unwrap(),
        Some(vec![vec![1, 2, 3]; 2])
    );
    framed.pop();
    assert_eq!(
        parse_packetized_speex_stream(framed, 540, Some(1026)).unwrap(),
        Some(vec![vec![1, 2, 3]; 2])
    );
    assert!(parse_packetized_speex_stream(vec![], 0, None).is_err());
    assert!(parse_packetized_speex_stream(vec![], 540, Some(0)).is_err());
    for (quality, size) in [10, 15, 20, 20, 28, 28, 38, 38, 46, 46]
        .into_iter()
        .enumerate()
    {
        assert_eq!(speex_packet_size(quality as u8 + 1, None).unwrap(), size);
        assert_eq!(
            split_raw_speex_packets(vec![7; size as usize * 2], quality as u8 + 1, None)
                .unwrap()
                .unwrap()
                .len(),
            2
        );
    }
    assert_eq!(speex_packet_size(0, None).unwrap(), 20);
    assert_eq!(speex_packet_size(3, Some(42)).unwrap(), 42);
    assert!(split_raw_speex_packets(vec![0; 21], 3, None)
        .unwrap()
        .is_none());
    assert!(split_raw_speex_packets(vec![0; 20], 3, Some(0)).is_err());
    assert!(is_ogg_speex(export_ogg_speex(packets, 16000, 9).unwrap()));
    assert!(!is_ogg_speex(vec![0; 100]));
    assert!(is_wav(
        export_wav_pcm(
            vec![0; 640],
            PcmFormat {
                sample_rate_hz: 16000,
                channels: 1,
                bit_depth: 16
            }
        )
        .unwrap()
    ));
    assert!(!is_wav(b"RIFF\0\0\0\0".to_vec()));
}
#[test]
fn speex_decoded_pcm_keeps_first_frame_per_packet() {
    let format = PcmFormat {
        sample_rate_hz: 16000,
        channels: 1,
        bit_depth: 16,
    };
    let pcm = [vec![1; 640], vec![9; 640], vec![2; 640], vec![8; 640]].concat();
    assert_eq!(
        normalize_decoded_speex_pcm(pcm.clone(), 2, format.clone()).unwrap(),
        [vec![1; 640], vec![2; 640]].concat()
    );
    assert_eq!(
        normalize_decoded_speex_pcm(pcm.clone(), 0, format.clone()).unwrap(),
        pcm
    );
    let short = vec![1; 600];
    assert_eq!(
        normalize_decoded_speex_pcm(short.clone(), 1, format.clone()).unwrap(),
        short
    );
    let irregular = vec![1; 1500];
    assert_eq!(
        normalize_decoded_speex_pcm(irregular.clone(), 2, format.clone()).unwrap(),
        irregular
    );
    assert!(normalize_decoded_speex_pcm(
        vec![],
        1,
        PcmFormat {
            sample_rate_hz: 0,
            ..format
        }
    )
    .is_err());
}
#[test]
fn record_result_and_gesture_names() {
    assert_eq!(
        parse_audio_record_result(vec![0, 0, 0, 0, 0, 5]).unwrap(),
        5
    );
    assert!(matches!(
        parse_audio_record_result(vec![0, 7]),
        Err(CoreError::Device { code: 7 })
    ));
    assert!(parse_audio_record_result(vec![0, 0, 0, 0, 0]).is_err());
    assert!(parse_audio_record_result(vec![0, 0, 0, 0, 0, 5, 0]).is_err());
    for (id, name) in ["idle", "rotate_back", "rotate_front", "wave"]
        .iter()
        .enumerate()
    {
        assert_eq!(sensor_gesture_name(id as u8), *name);
    }
    assert_eq!(sensor_gesture_name(255), "unknown(255)");
}
#[test]
fn crc_invalid_stream_frame_is_dropped_with_neighbors_at_every_split() {
    let first = encode_packet(1, b"first".to_vec()).unwrap();
    let last = encode_packet(2, b"last".to_vec()).unwrap();
    let mut bad = encode_packet(3, encode_packet(9, b"embedded magic".to_vec()).unwrap()).unwrap();
    *bad.last_mut().unwrap() ^= 1;
    assert!(decode_packet(bad.clone()).is_err());
    let wire = [first, bad.clone(), last].concat();
    for split in 0..=wire.len() {
        let decoder = PacketDecoder::new();
        let a = decoder.feed_batch(wire[..split].to_vec());
        let b = decoder.feed_batch(wire[split..].to_vec());
        assert!(a.fault.is_none() && b.fault.is_none());
        assert_eq!(a.dropped_crc_packets + b.dropped_crc_packets, 1);
        let packets = [a.packets, b.packets].concat();
        assert_eq!(
            packets.iter().map(|p| p.command).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(packets[0].body, b"first");
        assert_eq!(packets[1].body, b"last");
        assert_eq!(decoder.buffer_len(), 0);
    }
    let decoder = PacketDecoder::new();
    for byte in wire {
        assert!(decoder.feed(vec![byte]).is_ok());
    }
    assert_eq!(decoder.buffer_len(), 0);
    let decoder = PacketDecoder::new();
    assert!(decoder.feed(bad).unwrap().is_empty());
    assert_eq!(
        decoder.feed(encode_packet(2, vec![]).unwrap()).unwrap()[0].command,
        2
    );
}
#[test]
fn crc_drop_waits_for_complete_frame_and_counts_each_drop() {
    let mut bad = encode_packet(3, b"corrupted".to_vec()).unwrap();
    *bad.last_mut().unwrap() ^= 1;
    let decoder = PacketDecoder::new();
    let a = decoder.feed_batch(bad[..bad.len() - 1].to_vec());
    assert_eq!(a.dropped_crc_packets, 0);
    assert!(a.fault.is_none());
    assert_eq!(decoder.buffer_len(), bad.len() as u64 - 1);
    let b = decoder.feed_batch([&bad[bad.len() - 1..], bad.as_slice()].concat());
    assert_eq!(b.dropped_crc_packets, 2);
    assert!(b.packets.is_empty() && b.fault.is_none());
    assert_eq!(decoder.buffer_len(), 0);
}
#[test]
fn crc_recovery_does_not_hide_other_protocol_faults() {
    let mut bad_crc = encode_packet(3, vec![1]).unwrap();
    bad_crc[11] ^= 1;
    let mut version = encode_packet(1, vec![1]).unwrap();
    version[2] = 5;
    for bad in [version, vec![63, 0, 4, 1, 1, 255, 255, 255, 255, 0, 0]] {
        let decoder = PacketDecoder::new();
        let batch = decoder.feed_batch([bad_crc.clone(), bad].concat());
        assert!(batch.fault.is_some());
        assert_eq!(batch.dropped_crc_packets, 1);
        assert!(batch.packets.is_empty());
    }
}
#[test]
fn recording_config_respects_advertised_ranges() {
    let c = RecordingConfig {
        output: 1,
        disconnect_policy: 1,
        channels: 2,
        bitrate: 32000,
        gain_left_q8: 256,
        gain_right_q8: 256,
        max_duration_ms: 60000,
    };
    let mut l = RecordingLimits {
        sample_rate_hz: 16000,
        frame_duration_ms: 20,
        channels_mask: 6,
        outputs_mask: 15,
        continue_outputs_mask: 10,
        bitrate_min: 8000,
        bitrate_max: 32000,
        gain_min_q8: 0,
        gain_max_q8: 1024,
        duration_min_ms: 20,
        duration_max_ms: 60000,
        duration_step_ms: 20,
    };
    validate_recording_config(c.clone(), l.clone()).unwrap();
    let mut unlimited = c.clone();
    unlimited.max_duration_ms = 0;
    validate_recording_config(unlimited, l.clone()).unwrap();
    let mut misaligned = c.clone();
    misaligned.max_duration_ms = 21;
    assert!(validate_recording_config(misaligned, l.clone()).is_err());
    let mut below_min = c.clone();
    below_min.max_duration_ms = 20;
    let mut restricted = l.clone();
    restricted.duration_min_ms = 40;
    assert!(validate_recording_config(below_min, restricted).is_err());
    l.bitrate_max = 16000;
    assert!(validate_recording_config(c, l).is_err());
}
#[test]
fn audio_start_requests_preserve_reserved_field() {
    for request in [
        DeviceRequest::AudioInfo { file_index: 5 },
        DeviceRequest::AudioQuick { file_index: 5 },
    ] {
        assert_eq!(prepare_request(request).unwrap().body, [0, 0, 0, 0, 0, 5]);
    }
}
#[test]
fn unlimited_config_roundtrips_like_python_sdk() {
    let body = vec![
        0, 0, 1, 0, 0, 2, 0, 0, 0x7d, 0, 1, 0, 1, 0, 0, 0, 0x3e, 0x80, 0, 20, 3, 1, 1, 1, 0, 0, 0,
        0x0b, 0xb9, 16, 0, 100, 8, 0,
    ];
    let c = parse_recording_config(body).unwrap();
    assert_eq!(c.max_duration_ms, 0);
    assert_eq!(&recording_config_body(c).unwrap()[12..], &[0, 0]);
}
