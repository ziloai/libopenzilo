// SPDX-License-Identifier: MPL-2.0
use libopenzilo::*;

fn ring(cpu: &str, serial: &str) -> DeviceInfo {
    DeviceInfo {
        firmware_version: "synthetic".into(),
        model: "Synthetic ring".into(),
        system_time: 0,
        audio_storage_total: 0,
        audio_storage_available: 0,
        battery_percent: 0,
        battery_charging: false,
        sn: serial.into(),
        cpuid: cpu.into(),
    }
}

#[test]
fn simultaneous_links_isolate_identical_requests_and_disconnects() {
    let bindings = RingBindings::new(8).unwrap();
    let a_info = ring("test-a", "");
    let b_info = ring("test-b", "");
    bindings.bind("a".into(), a_info.clone()).unwrap();
    bindings.bind("b".into(), b_info.clone()).unwrap();
    let a = bindings.attach("a".into(), a_info).unwrap();
    let b = bindings.attach("b".into(), b_info).unwrap();
    let ta = a.engine.begin(0x503, 0x504, vec![], 0, 10, None).unwrap();
    let tb = b.engine.begin(0x503, 0x504, vec![], 0, 10, None).unwrap();
    assert_eq!(ta.id, tb.id);
    let bytes = encode_packet(0x504, vec![0, 0]).unwrap();
    assert!(
        matches!(&a.engine.receive(bytes.clone(), a.generation, 1).unwrap()[..], [Delivery::Response { request_id, .. }] if *request_id == ta.id)
    );
    assert_eq!(b.engine.next_deadline(), Some(10));
    assert!(bindings.detach(a.id, a.attachment).is_empty());
    assert!(
        matches!(&b.engine.receive(bytes, b.generation, 2).unwrap()[..], [Delivery::Response { request_id, .. }] if *request_id == tb.id)
    );
}

#[test]
fn stale_attachment_cannot_close_reconnected_ring_or_complete_its_requests() {
    let bindings = RingBindings::new(8).unwrap();
    let info = ring("test-a", "");
    bindings.bind("a".into(), info.clone()).unwrap();
    let first = bindings.attach("a".into(), info.clone()).unwrap();
    first
        .engine
        .begin(0x503, 0x504, vec![], 0, 10, None)
        .unwrap();
    assert!(matches!(
        bindings.unbind("a".into()),
        Err(CoreError::Busy { .. })
    ));
    assert!(matches!(
        &bindings.detach("a".into(), first.attachment)[..],
        [Delivery::Failure {
            code: ErrorCode::Disconnected,
            ..
        }]
    ));
    let second = bindings.attach("a".into(), info).unwrap();
    assert!(bindings.detach("a".into(), first.attachment).is_empty());
    assert_eq!(
        bindings.session("a".into()).unwrap().attachment,
        second.attachment
    );
    let ticket = second
        .engine
        .begin(0x503, 0x504, vec![], 0, 10, None)
        .unwrap();
    let bytes = encode_packet(0x504, vec![0, 0]).unwrap();
    assert!(first
        .engine
        .receive(bytes.clone(), first.generation, 1)
        .unwrap()
        .is_empty());
    assert!(
        matches!(&second.engine.receive(bytes, second.generation, 1).unwrap()[..], [Delivery::Response { request_id, .. }] if *request_id == ticket.id)
    );
    bindings.detach(second.id, second.attachment);
    bindings.unbind("a".into()).unwrap();
    assert!(bindings.list().is_empty());
}

#[test]
fn restored_bindings_reject_identity_mismatch_and_duplicate_transport() {
    let bindings = RingBindings::new(8).unwrap();
    let saved = bindings.bind("a".into(), ring("", "test-serial")).unwrap();
    let restored = RingBindings::new(8).unwrap();
    restored.restore(saved.clone()).unwrap();
    assert_eq!(restored.list(), vec![saved]);
    assert!(restored
        .bind("duplicate".into(), ring("test-cpu", "test-serial"))
        .is_err());
    assert!(matches!(
        restored.attach("a".into(), ring("", "wrong-serial")),
        Err(CoreError::Incompatible { .. })
    ));
    let updated = ring("test-cpu", "test-serial");
    restored.attach("a".into(), updated.clone()).unwrap();
    assert!(matches!(
        restored.attach("a".into(), updated),
        Err(CoreError::Busy { .. })
    ));
}
