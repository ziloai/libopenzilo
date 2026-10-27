// SPDX-License-Identifier: MPL-2.0
// Synthetic keys/payloads only: this does not contain a usable firmware image.
use libopenzilo::*;
use p256::{
    ecdsa::{signature::Signer, Signature, SigningKey},
    pkcs8::EncodePublicKey,
};
use sha2::{Digest, Sha256};
use std::io::{Cursor, Write};
fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}
fn bundle(profile: &str) -> (Vec<u8>, String) {
    let key = SigningKey::from_bytes((&[1u8; 32]).into()).unwrap();
    let public = key
        .verifying_key()
        .to_public_key_pem(Default::default())
        .unwrap();
    let id = hex(&Sha256::digest(
        key.verifying_key().to_public_key_der().unwrap().as_bytes(),
    ));
    let mut payload = b"RINGENC1".to_vec();
    payload.extend(1u32.to_le_bytes());
    payload.extend(5u32.to_le_bytes());
    payload.extend([0u8; 16]);
    payload.extend(b"FAKE!");
    let raw=serde_json::to_vec(&serde_json::json!({"schema":1,"algorithm":"ECDSA-P256-SHA256","key_id":id,"image":"app","version":"V2.001.0002.0002","model":"fixture-ring","board":"fixture/board","profile":profile,"security_version":1,"source_versions":["V2.001.0002.0001"],"payload":{"file":"app.ota.bin","format":"RINGENC1","size":payload.len(),"sha256":hex(&Sha256::digest(&payload))}})).unwrap();
    let mut message = b"OpenZilo firmware bundle v1\0".to_vec();
    message.extend(&raw);
    let signature: Signature = key.sign(&message);
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in [
        ("manifest.json", raw),
        ("manifest.sig", signature.to_der().as_bytes().to_vec()),
        ("app.ota.bin", payload),
    ] {
        archive
            .start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        archive.write_all(&data).unwrap();
    }
    (archive.finish().unwrap().into_inner(), public)
}
fn info(version: &str) -> DeviceInfo {
    DeviceInfo {
        firmware_version: version.into(),
        system_time: 0,
        audio_storage_total: 0,
        audio_storage_available: 0,
        battery_percent: 90,
        battery_charging: false,
        sn: "synthetic".into(),
        cpuid: "synthetic".into(),
        model: "fixture-ring".into(),
    }
}
fn packet(command: u16, body: Vec<u8>) -> Packet {
    decode_packet(encode_packet(command, body).unwrap()).unwrap()
}
#[test]
fn signed_bundle_wrong_key_tamper_and_zip_limits() {
    let (b, key) = bundle("production");
    assert_eq!(
        inspect_firmware_bundle(b.clone(), key.clone())
            .unwrap()
            .payload
            .len(),
        37
    );
    let wrong = SigningKey::from_bytes((&[2u8; 32]).into())
        .unwrap()
        .verifying_key()
        .to_public_key_pem(Default::default())
        .unwrap();
    assert!(inspect_firmware_bundle(b.clone(), wrong).is_err());
    let mut bad = b.clone();
    let at = bad.windows(5).position(|w| w == b"FAKE!").unwrap();
    bad[at] ^= 1;
    assert!(inspect_firmware_bundle(bad, key.clone()).is_err());
    assert!(inspect_firmware_bundle(vec![0; 1024 * 1024], key.clone()).is_err());
    assert!(inspect_firmware_bundle(b[..40].to_vec(), key).is_err());
}
#[test]
fn ota_requires_transfer_reboot_identity_and_exact_version() {
    let (b, key) = bundle("production");
    let t = OtaTransfer::new(
        b,
        key,
        info("V2.001.0002.0001"),
        "fixture/board".into(),
        false,
    )
    .unwrap();
    assert_eq!(decode_packet(t.begin().unwrap()).unwrap().command, 1005);
    assert!(t.handle(packet(2201, vec![0, 0])).is_err());
    let mut request = vec![0, 0, 0, 2];
    request.extend(0u32.to_be_bytes());
    request.extend(4096u32.to_be_bytes());
    let step = t.handle(packet(2001, request)).unwrap();
    assert_eq!(step.sent, 37);
    assert_eq!(decode_packet(step.frame.unwrap()).unwrap().command, 2101);
    t.handle(packet(2201, vec![0, 0])).unwrap();
    assert_eq!(t.phase(), OtaPhase::AwaitingReboot);
    assert!(t.verify_running(info("V2.001.0002.0001")).is_err());
    let mut wrong = info("V2.001.0002.0002");
    wrong.cpuid = "another".into();
    assert!(t.verify_running(wrong).is_err());
    t.verify_running(info("V2.001.0002.0002")).unwrap();
    assert_eq!(t.phase(), OtaPhase::Verified);
}
#[test]
fn ota_compatibility_and_test_opt_in() {
    let (b, key) = bundle("test");
    assert!(OtaTransfer::new(
        b.clone(),
        key.clone(),
        info("V2.001.0002.0001"),
        "fixture/board".into(),
        false
    )
    .is_err());
    assert!(OtaTransfer::new(
        b.clone(),
        key.clone(),
        info("V2.001.0002.0001"),
        "wrong/board".into(),
        true
    )
    .is_err());
    assert!(OtaTransfer::new(
        b,
        key,
        info("V2.001.0002.0001"),
        "fixture/board".into(),
        true
    )
    .is_ok());
}
