// SPDX-License-Identifier: MPL-2.0
use crate::{Capabilities, CoreError, DeviceInfo, FeatureVersion, RecordingLimits, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ErrorCode {
    InvalidData,
    DeviceRejected,
    Incompatible,
    Busy,
    Timeout,
    Disconnected,
    Cancelled,
    Overflow,
    OutcomeUnknown,
    Unavailable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Recovery {
    None,
    Reconnect,
    ReconnectAndQueryStatus,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum CapabilitySource {
    Device,
    VerifiedLegacyProfile,
    Unknown,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct CapabilityProfile {
    pub source: CapabilitySource,
    pub capabilities: Option<Capabilities>,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct CommandContract {
    pub command: u16,
    pub feature: String,
    pub major: u8,
    pub min_minor: u8,
    pub enhanced: bool,
    pub changes_state: bool,
}
#[uniffi::export]
pub fn feature_contracts() -> Vec<FeatureVersion> {
    [
        "system",
        "time_sync",
        "audio_files",
        "recording_control",
        "live_audio",
        "imu",
        "key_events",
        "double_tap",
        "ota",
        "gesture",
        "connection_rssi",
    ]
    .iter()
    .map(|name| FeatureVersion {
        name: (*name).into(),
        major: if *name == "imu" { 2 } else { 1 },
        minor: 0,
    })
    .collect()
}
#[uniffi::export]
pub fn command_contract(command: u16) -> Option<CommandContract> {
    let (feature, enhanced, changes_state) = match command {
        0x101 | 0x105 | 0x301 | 0x303 => ("system", false, false),
        0x107 => ("connection_rssi", true, false),
        0x402 => ("time_sync", false, false),
        0x501 | 0x503 | 0x506 | 0x507 | 0x509 => ("audio_files", false, false),
        0x50b => ("audio_files", false, true),
        0x512 | 0x51a => ("recording_control", true, false),
        0x514 | 0x516 | 0x518 => ("recording_control", true, true),
        0x50d => ("live_audio", true, true),
        0x601 | 0x603 => ("imu", true, true),
        0x605 => ("imu", true, false),
        0x701 => ("double_tap", true, false),
        0x702 => ("gesture", true, false),
        0x703 | 0x704 => ("key_events", true, false),
        1005 => ("ota", true, true),
        _ => return None,
    };
    Some(CommandContract {
        command,
        feature: feature.into(),
        major: if feature == "imu" { 2 } else { 1 },
        min_minor: 0,
        enhanced,
        changes_state,
    })
}
#[uniffi::export]
pub fn check_command_contract(
    command: u16,
    features: Vec<FeatureVersion>,
    source: CapabilitySource,
    allow_unverified: bool,
) -> Result<()> {
    let Some(c) = command_contract(command) else {
        return Ok(());
    };
    if !c.enhanced || (allow_unverified && source == CapabilitySource::Unknown) {
        return Ok(());
    }
    if source == CapabilitySource::Unknown
        || !crate::supports_feature(features, c.feature.clone(), c.major, c.min_minor)
    {
        return Err(CoreError::Incompatible {
            detail: format!("{} contract is unavailable or unknown", c.feature),
        });
    }
    Ok(())
}
#[uniffi::export]
pub fn legacy_capability_profile(info: DeviceInfo, reason: String) -> CapabilityProfile {
    if info.firmware_version == "V2.001.0002.0000-prod-bccb2ffc" && info.model == "Open Zilo" {
        return CapabilityProfile {
            source: CapabilitySource::VerifiedLegacyProfile,
            capabilities: Some(Capabilities {
                firmware_version: info.firmware_version,
                model: info.model,
                board_target: "ring_dvt2/nrf54l15/cpuapp".into(),
                build_id: "bccb2ffcbbe18cc63f2b5d4954432d2c7046fe56".into(),
                boot_id: String::new(),
                protocol_versions: vec![4],
                features: feature_contracts()
                    .into_iter()
                    .filter(|f| f.name == "ota")
                    .collect(),
                recording_limits: None,
            }),
            reason: Some(reason),
        };
    }
    // Exact verified released build only; release strings are opaque identifiers.
    if info.firmware_version == "V2.001.0002.0001-prod-7fd1dfc9" && info.model == "Open Zilo" {
        let capabilities = Capabilities {
            firmware_version: info.firmware_version,
            model: info.model,
            board_target: "ring_dvt2/nrf54l15/cpuapp".into(),
            build_id: "7fd1dfc9e9c698c47b91e100580deddde66f210e".into(),
            boot_id: String::new(),
            protocol_versions: vec![4],
            features: feature_contracts()
                .into_iter()
                .filter(|f| !matches!(f.name.as_str(), "gesture" | "connection_rssi"))
                .collect(),
            recording_limits: Some(RecordingLimits {
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
            }),
        };
        CapabilityProfile {
            source: CapabilitySource::VerifiedLegacyProfile,
            capabilities: Some(capabilities),
            reason: Some(reason),
        }
    } else {
        CapabilityProfile {
            source: CapabilitySource::Unknown,
            capabilities: None,
            reason: Some(reason),
        }
    }
}
#[uniffi::export]
pub fn verify_capability_identity(info: DeviceInfo, caps: Capabilities) -> Result<()> {
    if info.model != caps.model
        || info.firmware_version != caps.firmware_version
        || !caps.protocol_versions.contains(&4)
    {
        return Err(CoreError::Incompatible {
            detail: "Capability identity or protocol differs from system information".into(),
        });
    }
    Ok(())
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct LibraryInfo {
    pub build_id: String,
    pub version: String,
    pub api_contract: u32,
    pub features: Vec<String>,
}
#[uniffi::export]
pub fn library_info() -> LibraryInfo {
    LibraryInfo {
        build_id: env!("LIBOPENZILO_BUILD_ID").into(),
        version: crate::core_version(),
        api_contract: 1,
        features: vec![
            "protocol_v4".into(),
            "signed_ota".into(),
            "imu_calibration_v1".into(),
            "ogg_opus".into(),
            "ogg_speex".into(),
            "wav_pcm".into(),
            "connection_rssi".into(),
            "speex_stream_parsing".into(),
            "imu_static_collection".into(),
            "auto_audio_transfer".into(),
        ],
    }
}
