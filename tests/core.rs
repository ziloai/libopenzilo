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

fn live_start() -> Packet {
    let mut body = Vec::new();
    for field in [1u32, 0, 99, 0] {
        body.extend(field.to_be_bytes());
    }
    body.extend([0x3e, 0x80, 2, 3]);
    packet(0x50f, body)
}

fn live_data(session: u32, sequence: u32, offset: u32, opus: &[u8]) -> Packet {
    let mut body = Vec::new();
    for field in [session, sequence, offset] {
        body.extend(field.to_be_bytes());
    }
    body.extend((opus.len() as u16).to_be_bytes());
    body.extend(opus);
    packet(0x510, body)
}

fn live_end(error: u16, session: u32, produced: u32, sent: u32, dropped: u32) -> Packet {
    let mut body = error.to_be_bytes().to_vec();
    for field in [session, 0, produced, sent, dropped] {
        body.extend(field.to_be_bytes());
    }
    packet(0x511, body)
}

#[test]
fn live_preview_reports_first_middle_and_trailing_gaps_in_order() {
    let v = LivePreviewValidator::new();
    assert!(matches!(
        v.accept(live_start()).unwrap().as_slice(),
        [LivePreviewEvent::Start { .. }]
    ));
    for (sequence, offset, opus, expected) in [
        (1, 5, &[0x7c][..], (0, 0, 1, 5)),
        (4, 12, &[0x7c, 0, 0][..], (2, 6, 2, 6)),
    ] {
        match v
            .accept(live_data(1, sequence, offset, opus))
            .unwrap()
            .as_slice()
        {
            [LivePreviewEvent::Gap { value: gap }, LivePreviewEvent::Data { value: data }] => {
                assert_eq!(gap.session_id, 1);
                assert_eq!(
                    (
                        gap.sequence,
                        gap.byte_offset,
                        gap.missing_frames,
                        gap.missing_bytes
                    ),
                    expected
                );
                assert_eq!(
                    (data.sequence, data.byte_offset, data.data.as_slice()),
                    (sequence, offset, opus)
                );
            }
            events => panic!("Unexpected preview events: {events:?}"),
        }
    }
    assert!(matches!(
        v.accept(live_data(1, 5, 15, &[0x7c])).unwrap().as_slice(),
        [LivePreviewEvent::Data { .. }]
    ));
    match v.accept(live_end(0, 1, 23, 5, 5)).unwrap().as_slice() {
        [LivePreviewEvent::Gap { value: gap }, LivePreviewEvent::End { value: end }] => {
            assert_eq!(
                (
                    gap.session_id,
                    gap.sequence,
                    gap.byte_offset,
                    gap.missing_frames,
                    gap.missing_bytes
                ),
                (1, 6, 16, 2, 7)
            );
            assert_eq!(
                (end.produced_bytes, end.sent_bytes, end.dropped_frames),
                (23, 5, 5)
            );
        }
        events => panic!("Unexpected preview events: {events:?}"),
    }
    assert!(v.accept(live_end(0, 1, 23, 5, 5)).is_err());
}

#[test]
fn live_preview_rejects_bad_data_and_closes() {
    let before_start = LivePreviewValidator::new();
    assert!(before_start.accept(live_data(1, 0, 0, &[0x7c])).is_err());
    for (session, sequence, bytes, toc) in [
        (1, 1, 1, 0x7c),       // duplicate
        (1, 0, 3, 0x7c),       // backward sequence
        (1, 3, 1, 0x7c),       // backward offset
        (2, 3, 3, 0x7c),       // wrong session
        (1, 2, 3, 0x7c),       // offset-only jump
        (1, 3, 2, 0x7c),       // sequence-only jump
        (1, 4, 3, 0x7c),       // too few missing bytes
        (1, 3, 103, 0x7c),     // too many missing bytes
        (1, 3000, 3000, 0x7c), // duration limit
        (1, 2, 2, 0x78),       // wrong Opus channels
    ] {
        let v = LivePreviewValidator::new();
        v.accept(live_start()).unwrap();
        v.accept(live_data(1, 0, 0, &[0x7c])).unwrap();
        v.accept(live_data(1, 1, 1, &[0x7c])).unwrap();
        assert!(v
            .accept(live_data(session, sequence, bytes, &[toc]))
            .is_err());
        assert!(v.accept(live_data(1, 2, 2, &[0x7c])).is_err());
    }
}

#[test]
fn live_preview_end_reconciles_only_device_drops() {
    for (session, produced, sent, dropped) in [
        (2, 6, 1, 1),
        (1, 6, 1, 0),
        (1, 5, 1, 1),
        (1, 6, 2, 1),
        (1, 7, 1, 1),
        (1, 6, 1, 2),
        (1, 107, 1, 2),
        (1, 3005, 1, 3000),
        (1, u32::MAX, 1, u32::MAX),
    ] {
        let v = LivePreviewValidator::new();
        v.accept(live_start()).unwrap();
        v.accept(live_data(1, 1, 5, &[0x7c])).unwrap();
        assert!(v
            .accept(live_end(0, session, produced, sent, dropped))
            .is_err());
        assert!(v.accept(live_end(0, 1, 6, 1, 1)).is_err());
    }

    let v = LivePreviewValidator::new();
    v.accept(live_start()).unwrap();
    assert!(matches!(
        v.accept(live_end(7, 1, 3, 0, 1)),
        Err(CoreError::Device { code: 7 })
    ));
    let v = LivePreviewValidator::new();
    v.accept(live_start()).unwrap();
    assert!(matches!(
        v.accept(live_end(0, 1, 3, 0, 1)).unwrap().as_slice(),
        [
            LivePreviewEvent::Gap {
                value: LiveGap {
                    sequence: 0,
                    byte_offset: 0,
                    missing_frames: 1,
                    missing_bytes: 3,
                    ..
                }
            },
            LivePreviewEvent::End { .. },
        ]
    ));

    let strict = LiveValidator::new();
    strict.accept(live_start()).unwrap();
    strict.accept(live_data(1, 0, 0, &[0x7c])).unwrap();
    assert!(strict.accept(live_end(0, 1, 4, 1, 1)).is_err());
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

#[test]
fn platform_independent_rules() {
    assert_eq!(device_error_message(5), "command not exist");
    assert_eq!(device_error_message(77), "device error 77");
    let spec = ble_transport_spec();
    assert_eq!(
        (spec.write_chunk_size, spec.ota_write_chunk_limit),
        (20, 244)
    );
    assert!(spec.service_uuid.starts_with("6E400001"));
    for name in [" ring ", "OPEN ZILO", "prefix Open Zilo-abc"] {
        assert!(is_compatible_ring_name(Some(name.into())), "{name}");
    }
    for name in ["earring", "ringing", "Open Zilo- "] {
        assert!(!is_compatible_ring_name(Some(name.into())), "{name}");
    }
    assert!(!is_compatible_ring_name(None));
    assert_eq!(normalize_ble_address("AA:bb-01".into()), "aabb01");
    assert!(ble_address_matches("aa-bb".into(), Some("AA:BB".into())));
    assert!(ble_address_matches("x".into(), None));
    assert!(!ble_address_matches("x".into(), Some("y".into())));
    assert_eq!(
        local_device_key("  ABC ".into()).unwrap(),
        local_device_key("abc".into()).unwrap()
    );
    assert_eq!(local_device_key("abc".into()).unwrap().len(), 64);
    assert!(local_device_key(" ".into()).is_err());
    assert!(boards_match("a//b".into(), "/a/b".into()));
    assert!(!boards_match("a/../b".into(), "a/../b".into()));
    assert!(!boards_match("".into(), "".into()));
}

fn speex_sine_packets(rate: u32, quality: u8, frames: usize) -> Vec<Vec<u8>> {
    let mut enc = oxideav_speex::SpeexStreamEncoder::new(rate, quality).unwrap();
    let n = enc.frame_samples();
    (0..frames)
        .map(|f| {
            let pcm: Vec<i16> = (0..n)
                .map(|i| {
                    let t = (f * n + i) as f64 / f64::from(rate);
                    (8000.0 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()) as i16
                })
                .collect();
            enc.encode_frame(&pcm).unwrap()
        })
        .collect()
}
fn pcm_energy(bytes: &[u8]) -> f64 {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| f64::from(i16::from_le_bytes(*b)).powi(2))
        .sum::<f64>()
}
#[test]
fn native_speex_decodes_all_legacy_container_forms() {
    for (rate, quality) in [(8000u32, 5u8), (16000, 8)] {
        let packets = speex_sine_packets(rate, quality, 10);
        let frame = (rate / 50) as usize;
        let format = PcmFormat {
            sample_rate_hz: rate,
            channels: 1,
            bit_depth: 16,
        };
        // Ogg
        let ogg = export_ogg_speex(packets.clone(), rate, 7).unwrap();
        let decoded = decode_speex_pcm(ogg, format.clone(), 3, None, 540, None).unwrap();
        assert_eq!(decoded.source_type, "ogg-speex");
        assert_eq!(decoded.pcm_bytes.len(), 10 * frame * 2);
        assert!(pcm_energy(&decoded.pcm_bytes) > 1e6);
        // Length-prefixed packets
        let mut wire = Vec::new();
        for p in &packets {
            wire.extend((p.len() as u16).to_le_bytes());
            wire.extend(p);
        }
        wire.extend([0, 0]);
        let by_packet = decode_speex_pcm(wire, format.clone(), 3, None, 540, None).unwrap();
        assert_eq!(by_packet.source_type, "packet-speex");
        assert_eq!(by_packet.packet_count, 10);
        assert_eq!(by_packet.pcm_bytes, decoded.pcm_bytes);
        // Raw equal-size packets
        let size = packets.iter().map(Vec::len).max().unwrap();
        if packets.iter().all(|p| p.len() == size) {
            let raw: Vec<u8> = packets.concat();
            let raw =
                decode_speex_pcm(raw, format.clone(), 3, Some(size as u32), 540, None).unwrap();
            assert_eq!(raw.source_type, "raw-speex");
            assert_eq!(raw.pcm_bytes, decoded.pcm_bytes);
        }
        // Streaming decoder matches the one-shot result
        let ring = RingSpeexDecoder::new(rate, 1).unwrap();
        let streamed: Vec<u8> = packets
            .iter()
            .flat_map(|p| ring.decode_packet(p.clone()).unwrap())
            .collect();
        assert_eq!(streamed, decoded.pcm_bytes);
        assert_eq!(ring.format().sample_rate_hz, rate);
    }
}
#[test]
fn native_speex_converts_channels_rate_and_depth() {
    let packets = speex_sine_packets(8000, 5, 10);
    let mut wire = Vec::new();
    for p in &packets {
        wire.extend((p.len() as u16).to_le_bytes());
        wire.extend(p);
    }
    let eight = decode_speex_pcm(
        wire.clone(),
        PcmFormat {
            sample_rate_hz: 8000,
            channels: 2,
            bit_depth: 8,
        },
        3,
        None,
        540,
        None,
    )
    .unwrap();
    assert_eq!(eight.pcm_bytes.len(), 10 * 160 * 2);
    assert!(eight.pcm_bytes.iter().any(|b| b.abs_diff(128) > 20));
    let wav = export_wav_pcm(eight.pcm_bytes, eight.format).unwrap();
    assert!(is_wav(wav));
    assert!(decode_speex_pcm(
        vec![],
        PcmFormat {
            sample_rate_hz: 8000,
            channels: 1,
            bit_depth: 16
        },
        3,
        None,
        540,
        None
    )
    .is_err());
    assert!(decode_speex_pcm(
        vec![9; 7],
        PcmFormat {
            sample_rate_hz: 8000,
            channels: 3,
            bit_depth: 16
        },
        3,
        None,
        540,
        None
    )
    .is_err());
    assert!(RingSpeexDecoder::new(44100, 1).is_err());
    assert!(RingSpeexDecoder::new(8000, 1)
        .unwrap()
        .decode_packet(vec![])
        .is_err());
}
