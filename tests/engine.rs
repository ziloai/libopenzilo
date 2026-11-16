// SPDX-License-Identifier: MPL-2.0
use libopenzilo::*;
fn packet(command: u16, body: Vec<u8>) -> Packet {
    decode_packet(encode_packet(command, body).unwrap()).unwrap()
}
fn u32s(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}
fn metadata(index: u32, size: u32) -> Packet {
    packet(0x504, [vec![0, 0], u32s(&[index, 100, size])].concat())
}
fn frame(offset: u32, data: &[u8], end: bool) -> Packet {
    packet(
        0x505,
        [
            vec![0, 0],
            u32s(&[7, offset, data.len() as u32]),
            vec![end as u8],
            data.to_vec(),
        ]
        .concat(),
    )
}
#[test]
fn valid_delivery_survives_following_bad_crc_or_error_packet() {
    for bad in [
        vec![63, 0, 4, 1, 2, 0, 0, 0, 1, 0, 0, 1],
        encode_packet(0x1ff, vec![0]).unwrap(),
    ] {
        let s = DeviceSession::new(8).unwrap();
        let g = s.open().unwrap();
        let a = s.begin(1, 2, vec![], 0, 10, None).unwrap();
        let b = s.begin(3, 4, vec![], 0, 10, None).unwrap();
        let mut bytes = encode_packet(2, vec![]).unwrap();
        bytes.extend(bad);
        let result = s.receive_direct(bytes, g, 1).unwrap();
        assert!(matches!(result[0],Delivery::Response {request_id,..} if request_id==a.id));
        assert!(result
            .iter()
            .any(|v| matches!(v,Delivery::Failure {request_id,..} if *request_id==b.id)));
        assert!(matches!(result.last(), Some(Delivery::Fault { .. })));
        assert!(s.begin(1, 2, vec![], 2, 10, None).is_err());
    }
}
#[test]
fn late_reply_cannot_complete_a_retry_and_generation_rejects_old_callbacks() {
    let s = DeviceSession::new(8).unwrap();
    let g = s.open().unwrap();
    s.begin(0x516, 0x517, vec![], 0, 10, None).unwrap();
    assert_eq!(s.next_deadline(), Some(10));
    assert_eq!(s.tick(10).len(), 1);
    assert!(s.reconnect_required());
    assert!(matches!(
        s.begin(0x516, 0x517, vec![], 11, 10, None),
        Err(CoreError::OutcomeUnknown)
    ));
    s.disconnect(g);
    let next = s.open().unwrap();
    let t = s.begin(0x516, 0x517, vec![], 11, 10, None).unwrap();
    assert!(s
        .receive_direct(encode_packet(0x517, vec![]).unwrap(), g, 12)
        .unwrap()
        .is_empty());
    assert!(
        matches!(s.receive_direct(encode_packet(0x517,vec![]).unwrap(),next,12).unwrap()[0],Delivery::Response {request_id,..} if request_id==t.id)
    );
}
#[test]
fn recording_state_distinguishes_unknown_finalizing_and_idle() {
    let e = DeviceEngine::new(8).unwrap();
    let g = e.open().unwrap();
    assert_eq!(e.device_state().recording, ActivityState::Unknown);
    assert!(e.require_ota_idle().is_err());
    let t = e.begin(0x516, 0x517, vec![], 0, 10, None).unwrap();
    e.tick(10);
    assert_eq!(e.device_state().recording, ActivityState::Unknown);
    assert!(e.cancel(t.id).is_none());
    e.begin(0x51a, 0x51b, vec![], 11, 10, None).unwrap();
    let mut status = vec![0, 0];
    status.extend(7u32.to_be_bytes());
    status.extend([3, 0, 0, 0]);
    status.extend(0u32.to_be_bytes());
    e.receive(encode_packet(0x51b, status).unwrap(), g, 12)
        .unwrap();
    assert_eq!(e.device_state().recording, ActivityState::Finalizing);
    e.begin(0x51a, 0x51b, vec![], 13, 10, None).unwrap();
    let mut status = vec![0, 0];
    status.extend(7u32.to_be_bytes());
    status.extend([4, 1, 0, 0]);
    status.extend(0u32.to_be_bytes());
    e.receive(encode_packet(0x51b, status).unwrap(), g, 14)
        .unwrap();
    assert_eq!(e.device_state().recording, ActivityState::Idle);
    e.require_ota_idle().unwrap();
}
#[test]
fn capabilities_require_exact_contract_and_keep_unknown_distinct() {
    let features = vec![FeatureVersion {
        name: "imu".into(),
        major: 1,
        minor: 99,
    }];
    assert!(
        check_command_contract(0x601, features.clone(), CapabilitySource::Device, true).is_err()
    );
    assert!(check_command_contract(0x601, features, CapabilitySource::Unknown, true).is_ok());
    assert_eq!(command_contract(0x601).unwrap().major, 2);
    assert!(check_command_contract(0x101, vec![], CapabilitySource::Unknown, false).is_ok());
}
#[test]
fn download_quick_reorders_recovers_and_requires_end_ack() {
    let d = DownloadTransfer::new(7, 64, true, 10, 100).unwrap();
    assert_eq!(d.begin(0).unwrap().request.unwrap().command, 0x509);
    d.accept(metadata(7, 4), 1).unwrap();
    d.accept(frame(2, &[3, 4], true), 2).unwrap();
    let step = d.accept(frame(0, &[1, 2], false), 3).unwrap();
    assert_eq!(step.request.unwrap().command, 0x507);
    assert!(d.finish().is_err());
    d.accept(packet(0x508, [vec![0, 0], u32s(&[7])].concat()), 4)
        .unwrap();
    assert_eq!(d.finish().unwrap(), [1, 2, 3, 4]);
}
#[test]
fn download_bounds_conflicts_stalls_and_total_deadline() {
    let d = DownloadTransfer::new(7, 3, false, 10, 50).unwrap();
    d.begin(0).unwrap();
    assert!(d.accept(metadata(7, 4), 1).is_err());
    let d = DownloadTransfer::new(7, 64, true, 10, 100).unwrap();
    d.begin(0).unwrap();
    d.accept(metadata(7, 4), 1).unwrap();
    d.accept(frame(0, &[1, 2], false), 2).unwrap();
    assert!(d.accept(frame(1, &[9], false), 3).is_err());
    let d = DownloadTransfer::new(7, 64, true, 10, 100).unwrap();
    d.begin(0).unwrap();
    d.accept(metadata(7, 4), 1).unwrap();
    for now in [11, 21, 31] {
        assert_eq!(d.tick(now).unwrap().request.unwrap().command, 0x506);
    }
    assert!(d.tick(41).is_err());
    let d = DownloadTransfer::new(7, 64, true, 10, 20).unwrap();
    d.begin(0).unwrap();
    d.accept(metadata(7, 4), 1).unwrap();
    assert!(matches!(d.tick(20), Err(CoreError::Timeout)));
}
#[test]
fn zero_length_download_still_confirms_end_and_cancel_is_terminal() {
    let d = DownloadTransfer::new(7, 64, false, 10, 100).unwrap();
    d.begin(0).unwrap();
    assert_eq!(
        d.accept(metadata(7, 0), 1)
            .unwrap()
            .request
            .unwrap()
            .command,
        0x507
    );
    assert!(d.finish().is_err());
    d.cancel();
    assert!(d
        .accept(packet(0x508, [vec![0, 0], u32s(&[7])].concat()), 2)
        .is_err());
}

#[test]
fn sink_backpressure_commits_only_after_write_and_requires_end_ack() {
    let sink = AudioSinkTransfer::new(7, 100, 100).unwrap();
    sink.begin(0).unwrap();
    sink.accept(metadata(7, 4), 1).unwrap();
    let chunk = sink.accept(frame(0, &[1, 2], false), 2).unwrap();
    assert!(chunk.request.is_none());
    assert_eq!(chunk.committed, 0);
    assert_eq!(chunk.chunk.unwrap().data, [1, 2]);
    let next = sink.acknowledge_chunk(0, true, 3).unwrap();
    assert_eq!(next.committed, 2);
    assert_eq!(next.request.unwrap().command, 0x506);
    sink.accept(frame(2, &[3, 4], true), 4).unwrap();
    let finishing = sink.acknowledge_chunk(2, true, 5).unwrap();
    assert!(!finishing.complete);
    assert_eq!(finishing.request.unwrap().command, 0x507);
    assert!(
        sink.accept(packet(0x508, [vec![0, 0], u32s(&[7])].concat()), 6)
            .unwrap()
            .complete
    );
}

#[test]
fn sink_failure_wrong_ack_and_total_timeout_are_terminal() {
    for (offset, accepted, now) in [(1, true, 3), (0, false, 3), (0, true, 100)] {
        let sink = AudioSinkTransfer::new(7, 100, 100).unwrap();
        sink.begin(0).unwrap();
        sink.accept(metadata(7, 4), 1).unwrap();
        sink.accept(frame(0, &[1, 2], false), 2).unwrap();
        assert!(sink.acknowledge_chunk(offset, accepted, now).is_err());
        assert!(sink.acknowledge_chunk(0, true, now).is_err());
    }
}

#[test]
fn stale_engine_disconnect_does_not_reset_current_connection() {
    let engine = DeviceEngine::new(8).unwrap();
    let old = engine.open().unwrap();
    engine.disconnect(old);
    let current = engine.open().unwrap();
    engine.begin(0x516, 0x517, vec![], 0, 10, None).unwrap();
    engine
        .receive(
            encode_packet(0x517, [vec![0, 0], u32s(&[7])].concat()).unwrap(),
            current,
            1,
        )
        .unwrap();
    assert_eq!(engine.device_state().recording, ActivityState::Active);
    engine.disconnect(old);
    assert_eq!(engine.device_state().recording, ActivityState::Active);
}

#[test]
fn malformed_start_ack_keeps_outcome_unknown_and_blocks_retry() {
    let engine = DeviceEngine::new(8).unwrap();
    let generation = engine.open().unwrap();
    engine.begin(0x516, 0x517, vec![], 0, 10, None).unwrap();
    let deliveries = engine
        .receive(encode_packet(0x517, vec![0, 0]).unwrap(), generation, 1)
        .unwrap();
    assert!(matches!(
        deliveries[0],
        Delivery::Failure {
            code: ErrorCode::InvalidData,
            ..
        }
    ));
    assert_eq!(engine.device_state().recording, ActivityState::Unknown);
    assert!(matches!(
        engine.begin(0x516, 0x517, vec![], 2, 10, None),
        Err(CoreError::OutcomeUnknown)
    ));
}

#[test]
fn rssi_invalid_reply_quarantines_until_reconnect() {
    for body in [vec![0, 0], vec![0, 0, 128], vec![0, 7, 0]] {
        let engine = DeviceEngine::new(8).unwrap();
        let generation = engine.open().unwrap();
        engine.begin(0x107, 0x108, vec![], 0, 10, None).unwrap();
        let deliveries = engine
            .receive(encode_packet(0x108, body).unwrap(), generation, 1)
            .unwrap();
        assert!(matches!(
            deliveries[0],
            Delivery::Failure {
                code: ErrorCode::InvalidData,
                ..
            }
        ));
        assert!(engine.reconnect_required());
        assert!(matches!(
            engine.begin(0x107, 0x108, vec![], 2, 10, None),
            Err(CoreError::OutcomeUnknown)
        ));
        engine.disconnect(generation);
        let generation = engine.open().unwrap();
        engine.begin(0x107, 0x108, vec![], 3, 10, None).unwrap();
        let deliveries = engine
            .receive(
                encode_packet(0x108, vec![0, 0, 191]).unwrap(),
                generation,
                4,
            )
            .unwrap();
        assert!(matches!(deliveries[0], Delivery::Response { .. }));
        assert!(!engine.reconnect_required());
        engine.begin(0x107, 0x108, vec![], 5, 10, None).unwrap();
        let deliveries = engine
            .receive(encode_packet(0x108, vec![0, 7]).unwrap(), generation, 6)
            .unwrap();
        assert!(matches!(deliveries[0], Delivery::Response { .. }));
        assert!(!engine.reconnect_required());
    }
}
#[test]
fn deadline_progress_never_extends_total_and_timeout_latches() {
    let budget = DeadlineBudget::new(0, 10, 25).unwrap();
    assert_eq!(budget.remaining(5).unwrap(), 5);
    budget.progress(5).unwrap();
    assert_eq!(budget.next_deadline(), 15);
    budget.progress(14).unwrap();
    budget.progress(23).unwrap();
    assert_eq!(budget.next_deadline(), 25);
    assert!(matches!(budget.remaining(25), Err(CoreError::Timeout)));
    assert!(budget.progress(26).is_err());
    assert!(DeadlineBudget::new(u64::MAX, 1, 1).is_err());
}
#[test]
fn distance_preserves_quality_clears_stale_and_rejects_old_generation() {
    let d = DistanceTracker::new(2000, 100.0).unwrap();
    d.open(2, true, 100);
    let observation = DistanceObservation {
        meters: 1.25,
        quality: DistanceQuality::Low,
        source: DistanceSource::Replay,
        measured_at_ms: 100,
        generation: 2,
    };
    assert_eq!(
        d.accept(observation.clone(), 100).unwrap().state,
        DistanceState::LowQuality
    );
    assert!(d.tick(2101).unwrap().observation.is_none());
    let mut old = observation.clone();
    old.generation = 1;
    old.measured_at_ms = 2200;
    assert_eq!(d.accept(old, 2200).unwrap().state, DistanceState::Stale);
    let mut bad = observation;
    bad.meters = f64::NAN;
    assert!(d.accept(bad, 2300).is_err());
    let fresh = DistanceObservation {
        meters: 2.0,
        quality: DistanceQuality::High,
        source: DistanceSource::Replay,
        measured_at_ms: 2300,
        generation: 2,
    };
    d.accept(fresh.clone(), 2300).unwrap();
    let older = DistanceObservation {
        meters: 9.0,
        quality: DistanceQuality::Low,
        measured_at_ms: 2299,
        ..fresh
    };
    let retained = d.accept(older, 2300).unwrap().observation.unwrap();
    assert_eq!(retained.meters, 2.0);
    assert_eq!(retained.quality, DistanceQuality::High);
    d.disconnect(2);
    assert_eq!(d.tick(2301).unwrap().state, DistanceState::Disconnected);
}
