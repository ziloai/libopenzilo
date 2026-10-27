// SPDX-License-Identifier: MPL-2.0
use crate::{
    encode_packet, invalid,
    protocol::{crc, Reader},
    DeviceInfo, Packet, Result,
};
use p256::{
    ecdsa::{signature::Verifier, Signature, VerifyingKey},
    pkcs8::{DecodePublicKey, EncodePublicKey},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Read},
    sync::Mutex,
};
const MAX_PAYLOAD: usize = 0x7a000 + 32;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    file: String,
    format: String,
    size: u64,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    algorithm: String,
    key_id: String,
    image: String,
    version: String,
    model: String,
    board: String,
    profile: String,
    security_version: u32,
    source_versions: Vec<String>,
    payload: Payload,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct FirmwareBundle {
    pub version: String,
    pub model: String,
    pub board: String,
    pub profile: String,
    pub security_version: u32,
    pub source_versions: Vec<String>,
    pub payload: Vec<u8>,
    pub signing_key_id: String,
}
fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}
#[uniffi::export]
pub fn inspect_firmware_bundle(
    bundle: Vec<u8>,
    trusted_public_key: String,
) -> Result<FirmwareBundle> {
    if bundle.is_empty() || bundle.len() > MAX_PAYLOAD + 32768 {
        return Err(invalid("Firmware bundle size exceeds limit"));
    }
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bundle)).map_err(|_| invalid("Invalid firmware ZIP"))?;
    if archive.len() != 3 {
        return Err(invalid(
            "Bundle must contain exactly manifest.json, manifest.sig, app.ota.bin",
        ));
    }
    let mut entries = std::collections::BTreeMap::new();
    for i in 0..3 {
        let mut f = archive
            .by_index(i)
            .map_err(|_| invalid("Invalid firmware ZIP"))?;
        let name = f.name().to_string();
        let limit = match name.as_str() {
            "manifest.json" => 16384,
            "manifest.sig" => 80,
            "app.ota.bin" => MAX_PAYLOAD,
            _ => return Err(invalid("Unexpected firmware ZIP member")),
        };
        if f.compression() != zip::CompressionMethod::Stored
            || f.size() == 0
            || f.size() > limit as u64
            || f.compressed_size() != f.size()
        {
            return Err(invalid("Invalid bundle member size or compression"));
        }
        let mut d = Vec::new();
        f.by_ref()
            .take(limit as u64 + 1)
            .read_to_end(&mut d)
            .map_err(|_| invalid("Invalid firmware ZIP"))?;
        if entries.insert(name, d).is_some() {
            return Err(invalid("Duplicate firmware ZIP member"));
        }
    }
    let key = VerifyingKey::from_public_key_pem(&trusted_public_key)
        .map_err(|_| invalid("Expected a trusted P-256 public key"))?;
    let raw = &entries["manifest.json"];
    let sig = Signature::from_der(&entries["manifest.sig"])
        .map_err(|_| invalid("Firmware bundle signature verification failed"))?;
    let mut signed = b"OpenZilo firmware bundle v1\0".to_vec();
    signed.extend(raw);
    key.verify(&signed, &sig)
        .map_err(|_| invalid("Firmware bundle signature verification failed"))?;
    let m: Manifest = serde_json::from_slice(raw)
        .map_err(|_| invalid("Invalid firmware manifest JSON or fields"))?;
    let der = key
        .to_public_key_der()
        .map_err(|_| invalid("Invalid public key"))?;
    let key_id = hex(&Sha256::digest(der.as_bytes()));
    if m.schema != 1 || m.algorithm != "ECDSA-P256-SHA256" || m.image != "app" || m.key_id != key_id
    {
        return Err(invalid("Unsupported firmware manifest or signing key ID"));
    }
    let version =
        regex::Regex::new(r"^V[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+(?:-(?:debug|prod)-[0-9a-fA-F]{8})?$")
            .unwrap();
    if m.version.len() > 63
        || !version.is_match(&m.version)
        || m.model.is_empty()
        || m.model.len() > 128
        || m.board.is_empty()
        || m.board.len() > 128
        || !matches!(m.profile.as_str(), "test" | "production")
        || !(1..=0x7fffffff).contains(&m.security_version)
        || m.source_versions.len() > 64
        || m.source_versions
            .iter()
            .any(|v| v.len() > 63 || !version.is_match(v))
    {
        return Err(invalid("Invalid firmware manifest metadata"));
    }
    let payload = entries.remove("app.ota.bin").unwrap();
    if m.payload.file != "app.ota.bin"
        || m.payload.format != "RINGENC1"
        || m.payload.size != payload.len() as u64
        || m.payload.sha256 != hex(&Sha256::digest(&payload))
    {
        return Err(invalid("Firmware payload size or SHA-256 mismatch"));
    }
    if payload.len() <= 32
        || &payload[..8] != b"RINGENC1"
        || u32::from_le_bytes(payload[8..12].try_into().unwrap()) != 1
        || u32::from_le_bytes(payload[12..16].try_into().unwrap()) as usize != payload.len() - 32
    {
        return Err(invalid("Invalid encrypted APP header"));
    }
    Ok(FirmwareBundle {
        version: m.version,
        model: m.model,
        board: m.board,
        profile: m.profile,
        security_version: m.security_version,
        source_versions: m.source_versions,
        payload,
        signing_key_id: key_id,
    })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum OtaPhase {
    Ready,
    Transferring,
    AwaitingReboot,
    Verified,
    Cancelled,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct OtaStep {
    pub frame: Option<Vec<u8>>,
    pub sent: u32,
    pub total: u32,
    pub transfer_complete: bool,
}
struct OtaState {
    bundle: FirmwareBundle,
    before: DeviceInfo,
    sent_end: Option<u32>,
    phase: OtaPhase,
}
#[derive(uniffi::Object)]
pub struct OtaTransfer {
    state: Mutex<OtaState>,
}
fn board(s: &str) -> Vec<&str> {
    s.split('/').filter(|s| !s.is_empty()).collect()
}
#[uniffi::export]
impl OtaTransfer {
    #[uniffi::constructor]
    pub fn new(
        bundle: Vec<u8>,
        trusted_public_key: String,
        before: DeviceInfo,
        board_target: String,
        allow_test: bool,
    ) -> Result<Self> {
        let b = inspect_firmware_bundle(bundle, trusted_public_key)?;
        if b.profile == "test" && !allow_test {
            return Err(invalid("Test firmware requires explicit opt-in"));
        }
        if b.model != before.model {
            return Err(invalid("Device model differs from signed manifest"));
        }
        if board_target.is_empty() || board(&board_target) != board(&b.board) {
            return Err(invalid(
                "Device board is unknown or differs from signed manifest",
            ));
        }
        if !b.source_versions.is_empty() && !b.source_versions.contains(&before.firmware_version) {
            return Err(invalid(
                "Starting firmware version is not supported by signed manifest",
            ));
        }
        if b.version == before.firmware_version {
            return Err(invalid(
                "Device already reports the target version; no update was started",
            ));
        }
        Ok(Self {
            state: Mutex::new(OtaState {
                bundle: b,
                before,
                sent_end: None,
                phase: OtaPhase::Ready,
            }),
        })
    }
    pub fn phase(&self) -> OtaPhase {
        self.state.lock().unwrap().phase
    }
    pub fn begin(&self) -> Result<Vec<u8>> {
        let mut s = self.state.lock().unwrap();
        if s.phase != OtaPhase::Ready {
            return Err(invalid("OTA already started"));
        }
        let mut b = 2u32.to_be_bytes().to_vec();
        b.extend((s.bundle.version.len() as u32).to_be_bytes());
        b.extend(s.bundle.version.as_bytes());
        b.extend((s.bundle.payload.len() as u32).to_be_bytes());
        b.extend(crc(&s.bundle.payload, 65535).to_be_bytes());
        s.phase = OtaPhase::Transferring;
        encode_packet(1005, b)
    }
    pub fn handle(&self, packet: Packet) -> Result<OtaStep> {
        let mut s = self.state.lock().unwrap();
        if s.phase != OtaPhase::Transferring {
            return Err(invalid("OTA is not transferring"));
        }
        let mut r = Reader::new(&packet.body);
        r.success()?;
        let total = s.bundle.payload.len() as u32;
        if packet.command == 1105 || packet.command == 2201 {
            r.end()?;
            if packet.command == 2201 {
                if s.sent_end != Some(total) {
                    return Err(invalid(
                        "OTA success arrived before the package was transferred",
                    ));
                }
                s.phase = OtaPhase::AwaitingReboot;
            }
            return Ok(OtaStep {
                frame: None,
                sent: s.sent_end.unwrap_or(0),
                total,
                transfer_complete: s.phase == OtaPhase::AwaitingReboot,
            });
        }
        if packet.command != 2001 || packet.body.len() != 12 {
            return Err(invalid("Invalid OTA data request length"));
        }
        let ty = r.u16()?;
        let offset = r.u32()?;
        let window = r.u32()?;
        if ty != 0 && ty != 2 {
            return Err(invalid("Unexpected OTA upgrade type"));
        }
        if window == 0 || window > 4096 || offset >= total {
            return Err(invalid("Invalid OTA offset or window size"));
        }
        if s.sent_end.is_some_and(|end| offset > end) {
            return Err(invalid("OTA request skipped bytes that were never sent"));
        }
        let end = offset.saturating_add(window).min(total);
        let mut b = vec![0, 0];
        b.extend(offset.to_be_bytes());
        b.extend((end - offset).to_be_bytes());
        b.extend(&s.bundle.payload[offset as usize..end as usize]);
        s.sent_end = Some(s.sent_end.unwrap_or(0).max(end));
        Ok(OtaStep {
            frame: Some(encode_packet(2101, b)?),
            sent: s.sent_end.unwrap(),
            total,
            transfer_complete: false,
        })
    }
    pub fn verify_running(&self, after: DeviceInfo) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        if s.phase != OtaPhase::AwaitingReboot {
            return Err(invalid("OTA transfer is not complete"));
        }
        verify_running_identity(s.before.clone(), after, s.bundle.version.clone())?;
        s.phase = OtaPhase::Verified;
        Ok(())
    }
    pub fn cancel(&self) {
        self.state.lock().unwrap().phase = OtaPhase::Cancelled;
    }
}

#[uniffi::export]
pub fn verify_running_identity(
    before: DeviceInfo,
    after: DeviceInfo,
    target_version: String,
) -> Result<()> {
    if after.model != before.model
        || (!before.cpuid.is_empty() && after.cpuid != before.cpuid)
        || (!before.sn.is_empty() && after.sn != before.sn)
    {
        return Err(invalid(
            "Reconnected device identity differs; installation is unconfirmed",
        ));
    }
    if after.firmware_version != target_version {
        return Err(invalid(
            "Firmware version verification failed; installation is unconfirmed",
        ));
    }
    Ok(())
}
