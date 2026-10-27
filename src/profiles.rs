// SPDX-License-Identifier: MPL-2.0
use crate::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum ProfileBinding {
    LocalLocator,
    DeviceIdentity,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct CalibrationProfile {
    pub binding: ProfileBinding,
    pub device_key: String,
    pub firmware_version: String,
    pub model: String,
    pub calibration: ImuCalibration,
    pub accel_validated: bool,
    pub gyro_validated: bool,
    pub created_at: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileFile {
    schema_version: u32,
    application: String,
    input_domain: String,
    bias_accel_unit: String,
    bias_gyro_unit: String,
    scale_unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    binding: Option<ProfileBinding>,
    device_key: String,
    firmware_version: String,
    model: String,
    calibration: ImuCalibration,
    accel_validated: bool,
    gyro_validated: bool,
    created_at: String,
}
fn digest(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
#[uniffi::export]
pub fn stable_device_key(info: DeviceInfo) -> Result<String> {
    if info.model.is_empty() || (info.cpuid.is_empty() && info.sn.is_empty()) {
        return Err(CoreError::Incompatible {
            detail: "A verified stable device identity is required for a portable profile".into(),
        });
    }
    // Length-delimited canonical identity, independent of BLE address and platform.
    let mut bytes = b"OpenZilo IMU profile identity v2\0".to_vec();
    for v in [&info.model, &info.cpuid, &info.sn] {
        bytes.extend((v.len() as u64).to_be_bytes());
        bytes.extend(v.as_bytes());
    }
    Ok(digest(&bytes))
}
#[uniffi::export]
pub fn validate_calibration_profile(profile: CalibrationProfile) -> Result<()> {
    validate_imu_parameters(profile.calibration.clone())?;
    if time::OffsetDateTime::parse(
        &profile.created_at,
        &time::format_description::well_known::Rfc3339,
    )
    .is_err()
    {
        return Err(invalid("Invalid profile creation date"));
    }
    if profile.device_key.len() != 64
        || !profile
            .device_key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || profile.firmware_version.is_empty()
        || profile.model.is_empty()
        || profile.created_at.is_empty()
    {
        return Err(invalid("Invalid profile identity or date"));
    }
    if !profile.accel_validated && !profile.gyro_validated {
        return Err(invalid(
            "A profile must have at least one validated component",
        ));
    }
    let c = profile.calibration;
    if !profile.accel_validated
        && (c.accel_bias_ug != [0, 0, 0] || c.accel_scale_ppm != [1000000; 3])
    {
        return Err(invalid(
            "Unvalidated accelerometer parameters must be identity",
        ));
    }
    if (!profile.gyro_validated && c.gyro_bias_udps != [0, 0, 0])
        || (profile.gyro_validated && c.gyro_range_dps == 0)
    {
        return Err(invalid("Invalid gyro profile validation"));
    }
    Ok(())
}
#[uniffi::export]
pub fn parse_calibration_profile(json: String) -> Result<CalibrationProfile> {
    if json.len() > 16384 {
        return Err(invalid("Calibration profile exceeds limit"));
    }
    let p: ProfileFile =
        serde_json::from_str(&json).map_err(|_| invalid("Invalid calibration profile schema"))?;
    if !matches!(p.schema_version, 1 | 2)
        || p.application != "software"
        || p.input_domain != "board_adc_counts"
        || p.bias_accel_unit != "micro_g"
        || p.bias_gyro_unit != "micro_deg_per_s"
        || p.scale_unit != "ppm"
        || (p.schema_version == 1 && p.binding.is_some())
        || (p.schema_version == 2 && p.binding.is_none())
    {
        return Err(invalid("Unsupported IMU profile schema, source or units"));
    }
    let profile = CalibrationProfile {
        binding: p.binding.unwrap_or(ProfileBinding::LocalLocator),
        device_key: p.device_key,
        firmware_version: p.firmware_version,
        model: p.model,
        calibration: p.calibration,
        accel_validated: p.accel_validated,
        gyro_validated: p.gyro_validated,
        created_at: p.created_at,
    };
    validate_calibration_profile(profile.clone())?;
    Ok(profile)
}
#[uniffi::export]
pub fn serialize_calibration_profile(profile: CalibrationProfile) -> Result<String> {
    validate_calibration_profile(profile.clone())?;
    // Local v1 files stay local; migration never guesses a stable identity.
    let stable = profile.binding == ProfileBinding::DeviceIdentity;
    let p = ProfileFile {
        schema_version: if stable { 2 } else { 1 },
        application: "software".into(),
        input_domain: "board_adc_counts".into(),
        bias_accel_unit: "micro_g".into(),
        bias_gyro_unit: "micro_deg_per_s".into(),
        scale_unit: "ppm".into(),
        binding: if stable { Some(profile.binding) } else { None },
        device_key: profile.device_key,
        firmware_version: profile.firmware_version,
        model: profile.model,
        calibration: profile.calibration,
        accel_validated: profile.accel_validated,
        gyro_validated: profile.gyro_validated,
        created_at: profile.created_at,
    };
    serde_json::to_string_pretty(&p)
        .map(|v| v + "\n")
        .map_err(|_| invalid("Invalid calibration profile"))
}
#[uniffi::export]
pub fn check_profile_device(
    profile: CalibrationProfile,
    info: DeviceInfo,
    local_device_key: Option<String>,
    config: ImuConfig,
    features: Vec<FeatureVersion>,
) -> Result<()> {
    validate_calibration_profile(profile.clone())?;
    require_raw_imu_source(features)?;
    let key = match profile.binding {
        ProfileBinding::DeviceIdentity => stable_device_key(info.clone())?,
        ProfileBinding::LocalLocator => local_device_key
            .ok_or_else(|| invalid("Local profile requires its original platform locator"))?,
    };
    if key != profile.device_key
        || info.model != profile.model
        || info.firmware_version != profile.firmware_version
        || config.accel_range_g != profile.calibration.accel_range_g
        || config.gyro_range_dps != profile.calibration.gyro_range_dps
    {
        return Err(CoreError::Incompatible {
            detail: "IMU profile device, firmware, model or ranges differ".into(),
        });
    }
    Ok(())
}
#[uniffi::export]
pub fn require_raw_imu_source(features: Vec<FeatureVersion>) -> Result<()> {
    if !supports_feature(features.clone(), "imu".into(), 2, 0)
        || features
            .iter()
            .any(|f| f.name == "imu_calibration" || f.name == "unknown_11")
    {
        return Err(CoreError::Incompatible { detail:"Software IMU correction requires an explicit raw IMU contract without firmware calibration".into() });
    }
    Ok(())
}
