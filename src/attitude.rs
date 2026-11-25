// SPDX-License-Identifier: MPL-2.0
//! Portable 6-DoF IMU attitude fusion, ring body-frame axis mapping and posture classification.
use crate::{invalid, PhysicalImuBatch, PhysicalImuSample, Result};
use std::sync::Mutex;

/// Right-handed axis presets mapping IMU sensor axes onto the canonical Ring Body Frame:
/// `+X` finger bore axis, `+Y` lateral side rim, `+Z` radial outward through top button/boss.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ImuAxisPreset {
    Standard,
    SwappedXy,
    ZBore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ImuAxisMapping {
    pub preset: ImuAxisPreset,
    pub invert_radial_z: bool,
}

#[uniffi::export]
pub fn default_ring_axis_mapping(board_target: Option<String>) -> ImuAxisMapping {
    let _ = board_target;
    ImuAxisMapping {
        preset: ImuAxisPreset::Standard,
        invert_radial_z: false,
    }
}

impl ImuAxisMapping {
    pub(crate) fn apply(&self, v: [f64; 3]) -> [f64; 3] {
        let base = match self.preset {
            ImuAxisPreset::Standard => v,
            ImuAxisPreset::SwappedXy => [v[1], -v[0], v[2]],
            ImuAxisPreset::ZBore => [v[2], v[1], -v[0]],
        };
        // 180-deg rotation around +X bore preserves right-handed det = +1.
        if self.invert_radial_z {
            [base[0], -base[1], -base[2]]
        } else {
            base
        }
    }
}

#[uniffi::export]
pub fn map_physical_imu_sample(
    sample: PhysicalImuSample,
    mapping: ImuAxisMapping,
) -> Result<PhysicalImuSample> {
    let a = vec3(&sample.accel_g, "accel_g")?;
    let g = vec3(&sample.gyro_dps, "gyro_dps")?;
    Ok(PhysicalImuSample {
        timestamp_ms: sample.timestamp_ms,
        accel_g: mapping.apply(a).into(),
        gyro_dps: mapping.apply(g).into(),
    })
}

/// Unit quaternion `(w, x, y, z)` rotating Ring Body Frame vectors into World ENU (`+Z` up).
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct Quaternion {
    pub w: f64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct EulerAngles {
    pub roll_deg: f64,
    pub pitch_deg: f64,
    pub yaw_deg: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RingPosture {
    HorizontalUpright,
    HorizontalFlipped,
    VerticalStandingEdge,
    VerticalBoreAxis,
    Tilted,
    DynamicMotion,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct AttitudeSnapshot {
    pub timestamp_ms: u32,
    pub quaternion: Quaternion,
    pub euler: EulerAngles,
    pub tilt_from_zenith_deg: f64,
    pub posture: RingPosture,
    pub accel_body_g: Vec<f64>,
    pub gyro_body_dps: Vec<f64>,
    pub gyro_bias_dps: Vec<f64>,
    pub accel_norm_g: f64,
    pub angular_speed_dps: f64,
    pub stationary: bool,
    pub sample_count: u64,
}

impl Quaternion {
    const IDENTITY: Self = Self {
        w: 1.0,
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    fn checked(self) -> Result<Self> {
        if ![self.w, self.x, self.y, self.z]
            .iter()
            .all(|v| v.is_finite())
        {
            return Err(invalid("Quaternion contains non-finite components"));
        }
        let n = (self.w * self.w + self.x * self.x + self.y * self.y + self.z * self.z).sqrt();
        if n <= 1e-9 {
            return Err(invalid("Zero-norm quaternion"));
        }
        Ok(Self {
            w: self.w / n,
            x: self.x / n,
            y: self.y / n,
            z: self.z / n,
        })
    }

    fn normalized(self) -> Self {
        self.checked().unwrap_or(Self::IDENTITY)
    }

    fn conjugate(self) -> Self {
        Self {
            w: self.w,
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }

    fn mul(self, rhs: Self) -> Self {
        Self {
            w: self.w * rhs.w - self.x * rhs.x - self.y * rhs.y - self.z * rhs.z,
            x: self.w * rhs.x + self.x * rhs.w + self.y * rhs.z - self.z * rhs.y,
            y: self.w * rhs.y - self.x * rhs.z + self.y * rhs.w + self.z * rhs.x,
            z: self.w * rhs.z + self.x * rhs.y - self.y * rhs.x + self.z * rhs.w,
        }
    }

    fn rotate_body_to_world(self, v: [f64; 3]) -> [f64; 3] {
        let qv = [self.x, self.y, self.z];
        let uv = cross(qv, v);
        let uuv = cross(qv, uv);
        [
            v[0] + 2.0 * (uv[0] * self.w + uuv[0]),
            v[1] + 2.0 * (uv[1] * self.w + uuv[1]),
            v[2] + 2.0 * (uv[2] * self.w + uuv[2]),
        ]
    }

    fn rotate_world_to_body(self, v: [f64; 3]) -> [f64; 3] {
        self.conjugate().rotate_body_to_world(v)
    }

    fn from_axis_angle(axis: [f64; 3], angle_rad: f64) -> Self {
        let n = norm3(axis);
        if n <= 1e-9 {
            return Self::IDENTITY;
        }
        let half = 0.5 * angle_rad;
        let s = half.sin() / n;
        Self {
            w: half.cos(),
            x: axis[0] * s,
            y: axis[1] * s,
            z: axis[2] * s,
        }
        .normalized()
    }

    fn from_gravity_up(body_up: [f64; 3]) -> Self {
        let n = norm3(body_up);
        if n <= 1e-6 {
            return Self::IDENTITY;
        }
        let u = [body_up[0] / n, body_up[1] / n, body_up[2] / n];
        let dot = u[2].clamp(-1.0, 1.0);
        if dot > 0.999_999 {
            return Self::IDENTITY;
        }
        if dot < -0.999_999 {
            return Self {
                w: 0.0,
                x: 1.0,
                y: 0.0,
                z: 0.0,
            };
        }
        let axis = cross(u, [0.0, 0.0, 1.0]);
        Self {
            w: 1.0 + dot,
            x: axis[0],
            y: axis[1],
            z: axis[2],
        }
        .normalized()
    }

    fn euler_deg(self) -> EulerAngles {
        let sinr_cosp = 2.0 * (self.w * self.x + self.y * self.z);
        let cosr_cosp = 1.0 - 2.0 * (self.x * self.x + self.y * self.y);
        let roll_deg = sinr_cosp.atan2(cosr_cosp).to_degrees();

        let sinp = (2.0 * (self.w * self.y - self.z * self.x)).clamp(-1.0, 1.0);
        let pitch_deg = sinp.asin().to_degrees();

        let siny_cosp = 2.0 * (self.w * self.z + self.x * self.y);
        let cosy_cosp = 1.0 - 2.0 * (self.y * self.y + self.z * self.z);
        let yaw_deg = siny_cosp.atan2(cosy_cosp).to_degrees();

        EulerAngles {
            roll_deg,
            pitch_deg,
            yaw_deg,
        }
    }
}

#[uniffi::export]
pub fn quaternion_to_euler_deg(q: Quaternion) -> Result<EulerAngles> {
    Ok(q.checked()?.euler_deg())
}

#[uniffi::export]
pub fn quaternion_slerp(a: Quaternion, b: Quaternion, t: f64) -> Result<Quaternion> {
    if !t.is_finite() {
        return Err(invalid("Interpolation factor must be finite"));
    }
    let qa = a.checked()?;
    let qb = b.checked()?;
    let clamped = t.clamp(0.0, 1.0);
    let mut bw = qb.w;
    let mut bx = qb.x;
    let mut by = qb.y;
    let mut bz = qb.z;
    let mut cos_half = qa.w * bw + qa.x * bx + qa.y * by + qa.z * bz;
    if cos_half < 0.0 {
        bw = -bw;
        bx = -bx;
        by = -by;
        bz = -bz;
        cos_half = -cos_half;
    }
    if cos_half >= 0.9995 {
        return Ok(Quaternion {
            w: qa.w + clamped * (bw - qa.w),
            x: qa.x + clamped * (bx - qa.x),
            y: qa.y + clamped * (by - qa.y),
            z: qa.z + clamped * (bz - qa.z),
        }
        .normalized());
    }
    let half_theta = cos_half.clamp(-1.0, 1.0).acos();
    let sin_half = (1.0 - cos_half * cos_half).sqrt();
    let ra = ((1.0 - clamped) * half_theta).sin() / sin_half;
    let rb = (clamped * half_theta).sin() / sin_half;
    Ok(Quaternion {
        w: qa.w * ra + bw * rb,
        x: qa.x * ra + bx * rb,
        y: qa.y * ra + by * rb,
        z: qa.z * ra + bz * rb,
    }
    .normalized())
}

/// Column-major 4x4 rotation matrix in OpenGL Y-up coordinates (`(x_enu, y_enu, z_enu) -> (x, z, -y)`).
#[uniffi::export]
pub fn quaternion_to_opengl_matrix(q: Quaternion) -> Result<Vec<f32>> {
    let qn = q.checked()?;
    let (qw, qx, qy, qz) = (qn.w, qn.x, qn.z, -qn.y);
    let (xx, yy, zz) = (qx * qx, qy * qy, qz * qz);
    let (xy, xz, yz) = (qx * qy, qx * qz, qy * qz);
    let (wx, wy, wz) = (qw * qx, qw * qy, qw * qz);
    Ok([
        1.0 - 2.0 * (yy + zz),
        2.0 * (xy + wz),
        2.0 * (xz - wy),
        0.0,
        2.0 * (xy - wz),
        1.0 - 2.0 * (xx + zz),
        2.0 * (yz + wx),
        0.0,
        2.0 * (xz + wy),
        2.0 * (yz - wx),
        1.0 - 2.0 * (xx + yy),
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ]
    .into_iter()
    .map(|v| v as f32)
    .collect())
}

#[uniffi::export]
pub fn classify_ring_posture(
    q: Quaternion,
    accel_norm_g: f64,
    angular_speed_dps: f64,
) -> Result<RingPosture> {
    if !accel_norm_g.is_finite()
        || accel_norm_g < 0.0
        || !angular_speed_dps.is_finite()
        || angular_speed_dps < 0.0
    {
        return Err(invalid("Invalid acceleration norm or angular speed"));
    }
    let qn = q.checked()?;
    Ok(classify_checked(qn, accel_norm_g, angular_speed_dps))
}

fn classify_checked(qn: Quaternion, accel_norm_g: f64, angular_speed_dps: f64) -> RingPosture {
    const COS_30_DEG: f64 = 0.866_025_403_784_438_6;
    const SIN_28_DEG: f64 = 0.469_471_562_785_890_8;
    if angular_speed_dps > 95.0 || (accel_norm_g - 1.0).abs() > 0.24 {
        return RingPosture::DynamicMotion;
    }
    let bore_world = qn.rotate_body_to_world([1.0, 0.0, 0.0]);
    let side_world = qn.rotate_body_to_world([0.0, 1.0, 0.0]);
    let top_world = qn.rotate_body_to_world([0.0, 0.0, 1.0]);
    if bore_world[2].abs() >= COS_30_DEG {
        return RingPosture::VerticalBoreAxis;
    }
    if bore_world[2].abs() <= SIN_28_DEG {
        if top_world[2] >= COS_30_DEG {
            return RingPosture::HorizontalUpright;
        }
        if top_world[2] <= -COS_30_DEG {
            return RingPosture::HorizontalFlipped;
        }
        if side_world[2].abs() >= COS_30_DEG {
            return RingPosture::VerticalStandingEdge;
        }
    }
    RingPosture::Tilted
}

struct EstimatorState {
    mapping: ImuAxisMapping,
    mount: Quaternion,
    last_mapped_accel: Option<[f64; 3]>,
    kp: f64,
    q_raw: Quaternion,
    yaw_tare_rad: f64,
    full_tare: Option<Quaternion>,
    initialized: bool,
    last_timestamp_ms: Option<u32>,
    gyro_bias_sensor_dps: [f64; 3],
    stationary_duration_s: f64,
    sample_count: u64,
}

#[derive(uniffi::Object)]
pub struct ImuAttitudeEstimator {
    state: Mutex<EstimatorState>,
}

#[uniffi::export]
impl ImuAttitudeEstimator {
    #[uniffi::constructor]
    pub fn new(mapping: ImuAxisMapping, kp: f64) -> Result<Self> {
        check_kp(kp)?;
        Ok(Self {
            state: Mutex::new(EstimatorState {
                mapping,
                mount: Quaternion::IDENTITY,
                last_mapped_accel: None,
                kp,
                q_raw: Quaternion::IDENTITY,
                yaw_tare_rad: 0.0,
                full_tare: None,
                initialized: false,
                last_timestamp_ms: None,
                gyro_bias_sensor_dps: [0.0; 3],
                stationary_duration_s: 0.0,
                sample_count: 0,
            }),
        })
    }

    /// Calibrates the ring's flat-on-table mounting orientation from a measured static gravity
    /// vector (`accel_g` while the ring rests flat on its side face on a horizontal surface),
    /// mapping that gravity vector onto Ring Body `+X` (bore normal to the flat side face).
    pub fn calibrate_flat_table_mount(&self, static_accel_g: Vec<f64>) -> Result<Quaternion> {
        let raw = vec3(&static_accel_g, "static_accel_g")?;
        let mut s = self.state.lock().unwrap();
        let a = s.mapping.apply(raw);
        let n = norm3(a);
        if !(0.7..=1.3).contains(&n) {
            return Err(invalid(
                "Acceleration magnitude is not near 1g during flat calibration",
            ));
        }
        let mount = shortest_arc(a, [1.0, 0.0, 0.0]);
        s.mount = mount;
        Self::reset_pose(&mut s, true);
        Ok(mount)
    }

    /// Refines only the twist angle around the already-calibrated `+X` finger bore while the user
    /// wears the ring with the button facing upward, without disturbing the flat-face bore axis.
    pub fn calibrate_worn_button_up(&self) -> Result<Quaternion> {
        let mut s = self.state.lock().unwrap();
        let mapped = s
            .last_mapped_accel
            .ok_or_else(|| invalid("Receive IMU samples before aligning button direction"))?;
        let current = s.mount.rotate_body_to_world(mapped);
        let yz_norm = (current[1] * current[1] + current[2] * current[2]).sqrt();
        if yz_norm < 0.35 {
            return Err(invalid(
                "Hold your finger roughly horizontal with the button on top",
            ));
        }
        let twist_rad = current[1].atan2(current[2]);
        let twist = Quaternion::from_axis_angle([1.0, 0.0, 0.0], twist_rad);
        let updated = twist.mul(s.mount).normalized();
        s.mount = updated;
        Self::reset_pose(&mut s, true);
        Ok(updated)
    }

    pub fn calibrate_upright_mount(&self) -> Result<Quaternion> {
        let mut s = self.state.lock().unwrap();
        let a = s
            .last_mapped_accel
            .ok_or_else(|| invalid("Receive at least one IMU sample before upright alignment"))?;
        let n = norm3(a);
        if !(0.7..=1.3).contains(&n) {
            return Err(invalid(
                "Keep the ring still on a level surface during upright alignment",
            ));
        }
        let mount = Quaternion::from_gravity_up(a);
        s.mount = mount;
        Self::reset_pose(&mut s, true);
        Ok(mount)
    }

    pub fn set_mount_alignment(&self, mount: Quaternion) -> Result<()> {
        let checked = mount.checked()?;
        let mut s = self.state.lock().unwrap();
        s.mount = checked;
        Self::reset_pose(&mut s, true);
        Ok(())
    }

    pub fn clear_mount_alignment(&self) {
        let mut s = self.state.lock().unwrap();
        s.mount = Quaternion::IDENTITY;
        Self::reset_pose(&mut s, true);
    }

    pub fn set_axis_mapping(&self, mapping: ImuAxisMapping) {
        let mut s = self.state.lock().unwrap();
        s.mapping = mapping;
        Self::reset_pose(&mut s, true);
    }

    pub fn set_kp(&self, kp: f64) -> Result<()> {
        check_kp(kp)?;
        self.state.lock().unwrap().kp = kp;
        Ok(())
    }

    pub fn set_gyro_bias_sensor_dps(&self, bias_dps: Vec<f64>) -> Result<()> {
        let b = vec3(&bias_dps, "bias_dps")?;
        if b.iter().any(|v| v.abs() > 25.0) {
            return Err(invalid("Gyro bias exceeds ±25 deg/s limit"));
        }
        self.state.lock().unwrap().gyro_bias_sensor_dps = b;
        Ok(())
    }

    pub fn clear_gyro_bias(&self) {
        self.state.lock().unwrap().gyro_bias_sensor_dps = [0.0; 3];
    }

    pub fn zero_heading(&self) {
        let mut s = self.state.lock().unwrap();
        s.full_tare = None;
        s.yaw_tare_rad = s.q_raw.euler_deg().yaw_deg.to_radians();
    }

    pub fn zero_full_pose(&self) {
        let mut s = self.state.lock().unwrap();
        s.full_tare = Some(s.q_raw.conjugate());
    }

    pub fn reset(&self, keep_bias: bool) {
        let mut s = self.state.lock().unwrap();
        Self::reset_pose(&mut s, keep_bias);
    }

    pub fn accept_sample(&self, sample: PhysicalImuSample) -> Result<AttitudeSnapshot> {
        let mut s = self.state.lock().unwrap();
        Self::step(&mut s, &sample)
    }

    pub fn accept_batch(&self, batch: PhysicalImuBatch) -> Result<Vec<AttitudeSnapshot>> {
        if batch.frame_count as usize != batch.samples.len() {
            return Err(invalid("Physical IMU batch frame count mismatch"));
        }
        let mut s = self.state.lock().unwrap();
        batch
            .samples
            .iter()
            .map(|sample| Self::step(&mut s, sample))
            .collect()
    }
}

impl ImuAttitudeEstimator {
    fn reset_pose(s: &mut EstimatorState, keep_bias: bool) {
        s.q_raw = Quaternion::IDENTITY;
        s.yaw_tare_rad = 0.0;
        s.full_tare = None;
        s.initialized = false;
        s.last_timestamp_ms = None;
        s.stationary_duration_s = 0.0;
        s.sample_count = 0;
        if !keep_bias {
            s.gyro_bias_sensor_dps = [0.0; 3];
        }
    }

    fn step(s: &mut EstimatorState, sample: &PhysicalImuSample) -> Result<AttitudeSnapshot> {
        let raw_accel = vec3(&sample.accel_g, "accel_g")?;
        let raw_gyro = vec3(&sample.gyro_dps, "gyro_dps")?;
        let dt_s = match s.last_timestamp_ms {
            None => 0.01,
            Some(prev) => {
                let delta = sample.timestamp_ms.wrapping_sub(prev);
                if (1..=250).contains(&delta) {
                    f64::from(delta) / 1000.0
                } else {
                    0.01
                }
            }
        };
        s.last_timestamp_ms = Some(sample.timestamp_ms);
        s.sample_count = s.sample_count.saturating_add(1);

        let accel_mag = norm3(raw_accel);
        let pre_gyro = sub3(raw_gyro, s.gyro_bias_sensor_dps);
        if (accel_mag - 1.0).abs() < 0.035 && norm3(pre_gyro) < 2.0 {
            s.stationary_duration_s = (s.stationary_duration_s + dt_s).min(10.0);
            if s.stationary_duration_s >= 0.4 {
                let alpha = (dt_s * 0.8).clamp(0.001, 0.02);
                for (bias, &gyro) in s.gyro_bias_sensor_dps.iter_mut().zip(&raw_gyro) {
                    *bias += alpha * (gyro - *bias);
                }
            }
        } else {
            s.stationary_duration_s = 0.0;
        }

        let mapped_accel = s.mapping.apply(raw_accel);
        let mapped_gyro = s.mapping.apply(sub3(raw_gyro, s.gyro_bias_sensor_dps));
        let mapped_bias = s.mapping.apply(s.gyro_bias_sensor_dps);
        s.last_mapped_accel = Some(mapped_accel);

        let accel_body = s.mount.rotate_body_to_world(mapped_accel);
        let gyro_body_dps = s.mount.rotate_body_to_world(mapped_gyro);
        let bias_body_dps = s.mount.rotate_body_to_world(mapped_bias);

        if !s.initialized {
            if (0.5..=1.6).contains(&accel_mag) {
                s.q_raw = Quaternion::from_gravity_up(accel_body);
                s.yaw_tare_rad = s.q_raw.euler_deg().yaw_deg.to_radians();
                s.initialized = true;
            }
        } else {
            let mut wx = gyro_body_dps[0].to_radians();
            let mut wy = gyro_body_dps[1].to_radians();
            let mut wz = gyro_body_dps[2].to_radians();
            if (0.4..=2.0).contains(&accel_mag) {
                let dev = (accel_mag - 1.0) / 0.18;
                let kp_eff = s.kp * (-dev * dev).exp();
                if kp_eff > 1e-4 {
                    let measured_up = [
                        accel_body[0] / accel_mag,
                        accel_body[1] / accel_mag,
                        accel_body[2] / accel_mag,
                    ];
                    let estimated_up = s.q_raw.rotate_world_to_body([0.0, 0.0, 1.0]);
                    let err = cross(measured_up, estimated_up);
                    wx += kp_eff * err[0];
                    wy += kp_eff * err[1];
                    wz += kp_eff * err[2];
                }
            }
            let omega = norm3([wx, wy, wz]);
            let dq = if omega > 1e-7 {
                let half = 0.5 * omega * dt_s;
                let k = half.sin() / omega;
                Quaternion {
                    w: half.cos(),
                    x: wx * k,
                    y: wy * k,
                    z: wz * k,
                }
            } else {
                Quaternion {
                    w: 1.0,
                    x: 0.5 * wx * dt_s,
                    y: 0.5 * wy * dt_s,
                    z: 0.5 * wz * dt_s,
                }
            };
            s.q_raw = s.q_raw.mul(dq).normalized();
        }

        let q_display = match s.full_tare {
            Some(tare) => tare.mul(s.q_raw).normalized(),
            None => Quaternion::from_axis_angle([0.0, 0.0, 1.0], -s.yaw_tare_rad)
                .mul(s.q_raw)
                .normalized(),
        };
        let euler = q_display.euler_deg();
        let top_world = q_display.rotate_body_to_world([0.0, 0.0, 1.0]);
        let tilt_deg = top_world[2].clamp(-1.0, 1.0).acos().to_degrees();
        let angular_speed = norm3(gyro_body_dps);
        let posture = classify_checked(q_display, accel_mag, angular_speed);

        Ok(AttitudeSnapshot {
            timestamp_ms: sample.timestamp_ms,
            quaternion: q_display,
            euler,
            tilt_from_zenith_deg: tilt_deg,
            posture,
            accel_body_g: accel_body.into(),
            gyro_body_dps: gyro_body_dps.into(),
            gyro_bias_dps: bias_body_dps.into(),
            accel_norm_g: accel_mag,
            angular_speed_dps: angular_speed,
            stationary: s.stationary_duration_s >= 0.25,
            sample_count: s.sample_count,
        })
    }
}

fn check_kp(kp: f64) -> Result<()> {
    if !kp.is_finite() || !(0.0..=20.0).contains(&kp) {
        return Err(invalid("Mahony Kp must be within 0.0..=20.0"));
    }
    Ok(())
}

fn vec3(v: &[f64], label: &str) -> Result<[f64; 3]> {
    if v.len() != 3 || v.iter().any(|x| !x.is_finite()) {
        return Err(invalid(format!("{label} must contain 3 finite numbers")));
    }
    Ok([v[0], v[1], v[2]])
}

fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn norm3(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn shortest_arc(from: [f64; 3], to: [f64; 3]) -> Quaternion {
    let nf = norm3(from);
    let nt = norm3(to);
    if nf <= 1e-6 || nt <= 1e-6 {
        return Quaternion::IDENTITY;
    }
    let u = [from[0] / nf, from[1] / nf, from[2] / nf];
    let v = [to[0] / nt, to[1] / nt, to[2] / nt];
    let dot = (u[0] * v[0] + u[1] * v[1] + u[2] * v[2]).clamp(-1.0, 1.0);
    if dot > 0.999_999 {
        return Quaternion::IDENTITY;
    }
    if dot < -0.999_999 {
        let ortho = if u[2].abs() < 0.9 {
            cross(u, [0.0, 0.0, 1.0])
        } else {
            cross(u, [0.0, 1.0, 0.0])
        };
        return Quaternion::from_axis_angle(ortho, std::f64::consts::PI);
    }
    let axis = cross(u, v);
    Quaternion {
        w: 1.0 + dot,
        x: axis[0],
        y: axis[1],
        z: axis[2],
    }
    .normalized()
}
