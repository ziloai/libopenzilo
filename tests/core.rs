// SPDX-License-Identifier: MPL-2.0
use openzilo_core::*;
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
    let c = s.begin(1, 2, vec![], 12, 10, None).unwrap();
    assert!(s.cancel(c.id).is_some());
    let d = s.begin(1, 2, vec![], 12, 10, None).unwrap();
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
    assert!(s.receive(encode_packet(8, vec![]).unwrap(), g, 2).is_err());
    assert_eq!(s.take_events().len(), 1);
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
    let c = parse_capabilities(b).unwrap();
    assert!(supports_feature(c.features, "imu".into(), 2, 0));
    assert_eq!(c.recording_limits.unwrap().sample_rate_hz, 16000);
}
#[test]
fn decoder_can_drain_after_rejected_frame() {
    let d = PacketDecoder::new();
    let mut bad = encode_packet(1, vec![1]).unwrap();
    bad[11] ^= 1;
    bad.extend(encode_packet(2, vec![]).unwrap());
    assert!(d.feed(bad).is_err());
    assert_eq!(d.feed(vec![]).unwrap()[0].command, 2);
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
fn legacy_unlimited_config_is_readable_but_not_written() {
    let body = vec![
        0, 0, 1, 0, 0, 2, 0, 0, 0x7d, 0, 1, 0, 1, 0, 0, 0, 0x3e, 0x80, 0, 20, 3, 1, 1, 1, 0, 0, 0,
        0x0b, 0xb9, 16, 0, 100, 8, 0,
    ];
    let c = parse_recording_config(body).unwrap();
    assert_eq!(c.max_duration_ms, 0);
    assert!(recording_config_body(c).is_err());
}
