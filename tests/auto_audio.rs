// SPDX-License-Identifier: MPL-2.0
use libopenzilo::*;

fn packet(command: u16, body: Vec<u8>) -> Packet {
    decode_packet(encode_packet(command, body).unwrap()).unwrap()
}
fn words(values: &[u32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_be_bytes())
        .collect()
}
fn metadata(index: u32, size: u32) -> Packet {
    packet(0x504, [vec![0, 0], words(&[index, 100, size])].concat())
}
fn frame(index: u32, offset: u32, data: &[u8], end: bool) -> Packet {
    packet(
        0x505,
        [
            vec![0, 0],
            words(&[index, offset, data.len() as u32]),
            vec![u8::from(end)],
            data.to_vec(),
        ]
        .concat(),
    )
}
fn transfer() -> AutoAudioTransfer {
    let transfer = AutoAudioTransfer::new(1024, 10, 100).unwrap();
    let step = transfer.begin(0).unwrap();
    assert_eq!(step.phase, DownloadPhase::Receiving);
    assert_eq!(step.file_index, None);
    assert_eq!(step.total, None);
    assert!(step.request.is_none());
    transfer
}

#[test]
fn saved_notification_download_requires_end_ack() {
    let t = transfer();
    let notification = packet(0x50a, [vec![0, 0], words(&[5])].concat());
    let step = t.accept(notification, 1).unwrap();
    assert_eq!(step.request.unwrap().command, 0x503);
    assert_eq!(step.file_index, Some(5));
    assert_eq!(step.total, None);
    let step = t.accept(metadata(5, 6), 2).unwrap();
    assert_eq!(step.request.unwrap().command, 0x506);
    assert_eq!(step.total, Some(6));
    let step = t.accept(frame(5, 0, b"abcdef", true), 3).unwrap();
    assert_eq!(step.phase, DownloadPhase::Finishing);
    assert_eq!(step.request.unwrap().command, 0x507);
    assert!(t.finish().is_err());
    let step = t
        .accept(packet(0x508, [vec![0, 0], words(&[5])].concat()), 4)
        .unwrap();
    assert_eq!(step.phase, DownloadPhase::Complete);
    assert_eq!(t.finish().unwrap(), b"abcdef");
    assert_eq!(t.next_deadline(), None);
    assert!(t.accept(frame(5, 0, b"abcdef", true), 5).is_err());
    t.cancel();
    assert_eq!(t.finish().unwrap(), b"abcdef");
}

#[test]
fn legacy_contiguous_push_never_requests_metadata_or_end_ack() {
    let t = transfer();
    let step = t.accept(frame(5, 0, b"abc", false), 1).unwrap();
    assert_eq!(step.total, None);
    assert_eq!(step.received, 3);
    assert!(step.request.is_none());
    let step = t.accept(frame(5, 2, b"cdef", true), 2).unwrap();
    assert_eq!(step.phase, DownloadPhase::Complete);
    assert_eq!(step.total, Some(6));
    assert!(step.request.is_none());
    assert_eq!(t.finish().unwrap(), b"abcdef");
}

#[test]
fn legacy_out_of_order_end_recovers_metadata_and_missing_prefix() {
    let t = transfer();
    let step = t.accept(frame(5, 3, b"def", true), 1).unwrap();
    assert_eq!(step.phase, DownloadPhase::Metadata);
    assert_eq!(step.request.unwrap().command, 0x503);
    assert_eq!(step.total, Some(6));
    assert_eq!(step.received, 0);
    let step = t.accept(metadata(5, 6), 2).unwrap();
    let request = step.request.unwrap();
    assert_eq!(request.command, 0x506);
    assert_eq!(request.body, [0, 0, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0]);
    let step = t.accept(frame(5, 0, b"abc", false), 3).unwrap();
    assert_eq!(step.phase, DownloadPhase::Complete);
    assert_eq!(t.finish().unwrap(), b"abcdef");
}

#[test]
fn legacy_push_while_metadata_pending_waits_for_response() {
    let t = transfer();
    t.accept(frame(5, 3, b"def", true), 1).unwrap();
    let step = t.accept(frame(5, 0, b"abc", false), 2).unwrap();
    assert_eq!(step.phase, DownloadPhase::Metadata);
    assert!(step.request.is_none());
    assert!(t.finish().is_err());
    assert_eq!(
        t.accept(metadata(5, 6), 3).unwrap().phase,
        DownloadPhase::Complete
    );
    assert_eq!(t.finish().unwrap(), b"abcdef");
}

#[test]
fn idle_recovery_has_three_attempts_without_progress() {
    let t = transfer();
    t.accept(frame(5, 0, b"abc", false), 1).unwrap();
    assert_eq!(t.next_deadline(), Some(11));
    assert_eq!(t.tick(11).unwrap().request.unwrap().command, 0x503);
    t.accept(metadata(5, 6), 12).unwrap();
    assert_eq!(t.tick(22).unwrap().request.unwrap().command, 0x506);
    assert_eq!(t.tick(32).unwrap().request.unwrap().command, 0x506);
    assert!(t.tick(42).is_err());
    assert_eq!(t.next_deadline(), None);
    assert!(t.finish().is_err());
    assert!(t.accept(frame(5, 3, b"def", true), 43).is_err());
    assert!(t.begin(43).is_err());
}

#[test]
fn progress_resets_recovery_but_duplicates_do_not_extend_idle_deadline() {
    let t = transfer();
    t.accept(frame(5, 0, b"ab", false), 1).unwrap();
    t.tick(11).unwrap();
    t.accept(metadata(5, 6), 12).unwrap();
    t.tick(22).unwrap();
    t.tick(32).unwrap();
    let step = t.accept(frame(5, 2, b"cd", false), 33).unwrap();
    assert_eq!(step.received, 4);
    t.accept(frame(5, 0, b"abcd", false), 40).unwrap();
    assert_eq!(t.next_deadline(), Some(43));
    assert!(t.tick(43).unwrap().request.is_some());
    t.accept(frame(5, 4, b"ef", true), 44).unwrap();
    assert_eq!(t.finish().unwrap(), b"abcdef");
}

#[test]
fn wrong_identity_conflicting_overlap_and_metadata_fail_terminally() {
    for bad in [
        frame(6, 3, b"def", true),
        frame(5, 1, b"X", false),
        frame(5, 0, b"ab", true),
        frame(5, 3, b"", false),
    ] {
        let t = transfer();
        t.accept(frame(5, 0, b"abc", false), 1).unwrap();
        assert!(t.accept(bad, 2).is_err());
        assert_eq!(t.next_deadline(), None);
        assert!(t.finish().is_err());
    }
    for bad in [metadata(6, 6), metadata(5, 5), metadata(5, 1025)] {
        let t = transfer();
        t.accept(frame(5, 3, b"def", true), 1).unwrap();
        assert!(t.accept(bad, 2).is_err());
        assert!(t.finish().is_err());
    }
    let t = transfer();
    t.accept(frame(5, 3, b"def", false), 1).unwrap();
    assert!(t.accept(frame(5, 4, b"X", false), 2).is_err());
}

#[test]
fn limits_timeouts_clock_and_cancel_are_terminal() {
    for limits in [
        (0, 1, 1),
        (64 * 1024 * 1024 + 1, 1, 1),
        (1, 0, 1),
        (1, 1, 0),
    ] {
        assert!(AutoAudioTransfer::new(limits.0, limits.1, limits.2).is_err());
    }
    for bad in [frame(5, 1024, b"x", false), frame(5, u32::MAX, b"x", true)] {
        let t = transfer();
        assert!(t.accept(bad, 1).is_err());
        assert_eq!(t.next_deadline(), None);
    }
    let t = transfer();
    assert!(matches!(t.tick(10), Err(CoreError::Timeout)));
    assert!(t.accept(frame(5, 0, b"abc", true), 11).is_err());
    let t = transfer();
    t.accept(frame(5, 0, b"abc", false), 1).unwrap();
    assert!(t.accept(frame(5, 3, b"def", true), 0).is_err());
    let t = AutoAudioTransfer::new(1024, 100, 10).unwrap();
    t.begin(0).unwrap();
    t.accept(frame(5, 0, b"abc", false), 9).unwrap();
    assert!(matches!(t.tick(10), Err(CoreError::Timeout)));
    let t = transfer();
    t.accept(frame(5, 0, b"abc", false), 1).unwrap();
    t.cancel();
    assert!(t.accept(frame(5, 3, b"def", true), 2).is_err());
    assert_eq!(t.next_deadline(), None);
    assert!(t.finish().is_err());
    assert!(t.begin(2).is_err());
}

#[test]
fn metadata_timeout_and_begin_overflow_are_terminal() {
    let t = transfer();
    t.accept(frame(5, 3, b"def", true), 1).unwrap();
    assert!(matches!(t.tick(11), Err(CoreError::Timeout)));
    assert!(t.accept(metadata(5, 6), 12).is_err());
    assert_eq!(t.next_deadline(), None);
    let t = AutoAudioTransfer::new(1024, 10, 100).unwrap();
    assert!(t.begin(u64::MAX - 1).is_err());
    assert!(t.begin(0).is_err());
    assert_eq!(t.next_deadline(), None);
    assert!(t.finish().is_err());
}

#[test]
fn legacy_empty_end_and_definite_device_errors() {
    let t = transfer();
    assert_eq!(
        t.accept(frame(5, 0, b"", true), 1).unwrap().phase,
        DownloadPhase::Complete
    );
    assert!(t.finish().unwrap().is_empty());
    let t = transfer();
    t.accept(frame(5, 3, b"def", true), 1).unwrap();
    assert!(matches!(
        t.accept(packet(0x1ff, vec![5, 3, 0, 7]), 2),
        Err(CoreError::Device { code: 7 })
    ));
    assert_eq!(t.next_deadline(), None);
}
