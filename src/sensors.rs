// SPDX-License-Identifier: MPL-2.0
use crate::*;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct ImuCalibration {
    pub sensor_type: u8,
    pub accel_range_g: u16,
    pub gyro_range_dps: u16,
    pub accel_bias_ug: Vec<i32>,
    pub accel_scale_ppm: Vec<i32>,
    pub gyro_bias_udps: Vec<i32>,
}
#[uniffi::export]
pub fn validate_imu_parameters(c: ImuCalibration) -> Result<()> {
    check_calibration(&c)
}
#[uniffi::export]
pub fn identity_imu_calibration(config: ImuConfig) -> Result<ImuCalibration> {
    let calibration = ImuCalibration {
        sensor_type: 0,
        accel_range_g: config.accel_range_g,
        gyro_range_dps: config.gyro_range_dps,
        accel_bias_ug: vec![0; 3],
        accel_scale_ppm: vec![1000000; 3],
        gyro_bias_udps: vec![0; 3],
    };
    check_calibration(&calibration)?;
    Ok(calibration)
}
fn check_calibration(c: &ImuCalibration) -> Result<()> {
    if c.sensor_type > 3
        || ![2, 4, 8, 16].contains(&c.accel_range_g)
        || ![0, 125, 250, 500, 1000, 2000].contains(&c.gyro_range_dps)
    {
        return Err(invalid("Invalid sensor identity or ranges"));
    }
    for (v, low, high) in [
        (&c.accel_bias_ug, -200000, 200000),
        (&c.accel_scale_ppm, 800000, 1200000),
        (&c.gyro_bias_udps, -20000000, 20000000),
    ] {
        if v.len() != 3 || v.iter().any(|x| *x < low || *x > high) {
            return Err(invalid("Calibration parameters exceed supported bounds"));
        }
    }
    if c.gyro_range_dps == 0 && c.gyro_bias_udps.iter().any(|v| *v != 0) {
        return Err(invalid("Accelerometer-only sensor cannot have gyro bias"));
    }
    Ok(())
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct PhysicalImuSample {
    pub timestamp_ms: u32,
    pub accel_g: Vec<f64>,
    pub gyro_dps: Vec<f64>,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct PhysicalImuBatch {
    pub sequence_start: u32,
    pub frame_count: u16,
    pub samples: Vec<PhysicalImuSample>,
    pub applied_components: Vec<String>,
}
#[uniffi::export]
pub fn apply_imu_calibration(
    batch: ImuBatch,
    config: ImuConfig,
    calibration: ImuCalibration,
    accel: bool,
    gyro: bool,
) -> Result<PhysicalImuBatch> {
    check_calibration(&calibration)?;
    if config.accel_range_g != calibration.accel_range_g
        || config.gyro_range_dps != calibration.gyro_range_dps
    {
        return Err(invalid(
            "Calibration ranges differ from active IMU configuration",
        ));
    }
    if batch.frame_count as usize != batch.samples.len() || batch.sample_size != 16 {
        return Err(invalid("Invalid raw IMU batch"));
    }
    let samples = batch
        .samples
        .iter()
        .map(|s| {
            let a = [s.accel_x, s.accel_y, s.accel_z];
            let g = [s.gyro_x, s.gyro_y, s.gyro_z];
            PhysicalImuSample {
                timestamp_ms: s.timestamp_ms,
                accel_g: (0..3)
                    .map(|i| {
                        (f64::from(a[i]) * f64::from(config.accel_range_g) / 32768.0
                            - if accel {
                                f64::from(calibration.accel_bias_ug[i]) / 1e6
                            } else {
                                0.0
                            })
                            * if accel {
                                f64::from(calibration.accel_scale_ppm[i]) / 1e6
                            } else {
                                1.0
                            }
                    })
                    .collect(),
                gyro_dps: (0..3)
                    .map(|i| {
                        f64::from(g[i]) * f64::from(config.gyro_range_dps) / 32768.0
                            - if gyro {
                                f64::from(calibration.gyro_bias_udps[i]) / 1e6
                            } else {
                                0.0
                            }
                    })
                    .collect(),
            }
        })
        .collect();
    let mut applied_components = Vec::new();
    if accel {
        applied_components.push("accel".into());
    }
    if gyro && config.gyro_range_dps != 0 {
        applied_components.push("gyro".into());
    }
    Ok(PhysicalImuBatch {
        sequence_start: batch.sequence_start,
        frame_count: batch.frame_count,
        samples,
        applied_components,
    })
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct ImuStaticWindow {
    pub accel_g: Vec<f64>,
    pub gyro_dps: Vec<f64>,
    pub accel_std_g: f64,
    pub gyro_std_dps: f64,
    pub samples: u32,
}
fn check_window(w: &ImuStaticWindow) -> Result<()> {
    if w.accel_g.len() != 3
        || w.gyro_dps.len() != 3
        || w.accel_g.iter().chain(&w.gyro_dps).any(|v| !v.is_finite())
        || !w.accel_std_g.is_finite()
        || w.accel_std_g < 0.0
        || !w.gyro_std_dps.is_finite()
        || w.gyro_std_dps < 0.0
        || w.samples == 0
    {
        return Err(invalid("Invalid static window statistics"));
    }
    Ok(())
}
fn norm(v: &[f64]) -> f64 {
    v.iter().map(|x| x * x).sum::<f64>().sqrt()
}
fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}
fn std(v: &[f64]) -> f64 {
    let m = mean(v);
    (v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / v.len() as f64).sqrt()
}
#[uniffi::export]
pub fn imu_window_face(window: ImuStaticWindow) -> Result<String> {
    check_window(&window)?;
    let a = window.accel_g;
    let i = (0..3)
        .max_by(|i, j| a[*i].abs().partial_cmp(&a[*j].abs()).unwrap())
        .unwrap();
    let n = norm(&a);
    if n == 0.0 || a[i].abs() / n < 0.8 {
        return Err(invalid(
            "Orientation is ambiguous between two axes; adjust the case and let it settle",
        ));
    }
    Ok(format!(
        "{}{}",
        ["X", "Y", "Z"][i],
        if a[i] > 0.0 { "+" } else { "-" }
    ))
}
#[uniffi::export]
pub fn analyze_imu_static_window(
    samples: Vec<ImuSample>,
    config: ImuConfig,
    check_gyro: bool,
) -> Result<ImuStaticWindow> {
    if config.sample_rate_hz == 0
        || config.sample_rate_hz > 1000
        || ![2, 4, 8, 16].contains(&config.accel_range_g)
        || ![0, 125, 250, 500, 1000, 2000].contains(&config.gyro_range_dps)
    {
        return Err(invalid("Invalid IMU sampling configuration"));
    }
    if samples.len() < 20.max(config.sample_rate_hz as usize * 2) {
        return Err(invalid(
            "Not enough valid samples; keep the ring still for longer",
        ));
    }
    let mut axes = (0..6)
        .map(|_| Vec::with_capacity(samples.len()))
        .collect::<Vec<_>>();
    for s in &samples {
        for (i, v) in [
            s.accel_x, s.accel_y, s.accel_z, s.gyro_x, s.gyro_y, s.gyro_z,
        ]
        .iter()
        .enumerate()
        {
            if (i < 3 || check_gyro) && i32::from(*v).abs() >= 32760 {
                return Err(invalid(
                    "Sensor data is saturated; calibration cannot proceed",
                ));
            }
            axes[i].push(
                f64::from(*v)
                    * f64::from(if i < 3 {
                        config.accel_range_g
                    } else {
                        config.gyro_range_dps
                    })
                    / 32768.0,
            );
        }
    }
    let a: Vec<f64> = axes[..3].iter().map(|v| mean(v)).collect();
    let g: Vec<f64> = axes[3..].iter().map(|v| mean(v)).collect();
    let ast = axes[..3].iter().map(|v| std(v)).fold(0.0, f64::max);
    let gst = axes[3..].iter().map(|v| std(v)).fold(0.0, f64::max);
    if !(0.7 < norm(&a) && norm(&a) < 1.3) {
        return Err(invalid(
            "Acceleration magnitude is not near gravity; check mounting, units and sensor ranges",
        ));
    }
    if check_gyro && g.iter().any(|v| v.abs() > 20.0) {
        return Err(invalid(
            "Stationary angular velocity exceeds the supported bias range",
        ));
    }
    if ast > 0.008
        || (check_gyro && gst > 0.3)
        || axes[..3].iter().any(|v| {
            v.iter().copied().fold(f64::NEG_INFINITY, f64::max)
                - v.iter().copied().fold(f64::INFINITY, f64::min)
                > 0.06
        })
    {
        return Err(invalid(
            "Motion or vibration detected; keep the ring still and retry",
        ));
    }
    let half = samples.len() / 2;
    if axes.iter().enumerate().any(|(i, v)| {
        (i < 3 || check_gyro)
            && (mean(&v[..half]) - mean(&v[half..])).abs() > if i < 3 { 0.003 } else { 0.15 }
    }) {
        return Err(invalid(
            "Readings are still drifting; wait for them to stabilize and retry",
        ));
    }
    Ok(ImuStaticWindow {
        accel_g: a,
        gyro_dps: g,
        accel_std_g: ast,
        gyro_std_dps: gst,
        samples: samples.len() as u32,
    })
}
#[uniffi::export]
pub fn fit_imu_gyro_bias(
    mut previous: ImuCalibration,
    window: ImuStaticWindow,
) -> Result<ImuCalibration> {
    check_calibration(&previous)?;
    check_window(&window)?;
    if previous.gyro_range_dps == 0 {
        return Err(invalid("This sensor has no gyroscope"));
    }
    previous.gyro_bias_udps = window
        .gyro_dps
        .iter()
        .map(|v| (v * 1e6).round_ties_even() as i32)
        .collect();
    check_calibration(&previous)?;
    Ok(previous)
}
#[uniffi::export]
pub fn fit_imu_six_face_calibration(
    mut previous: ImuCalibration,
    windows: Vec<ImuStaticWindow>,
) -> Result<ImuCalibration> {
    check_calibration(&previous)?;
    let faces: std::collections::BTreeSet<_> = windows
        .iter()
        .map(|w| imu_window_face(w.clone()))
        .collect::<Result<_>>()?;
    if windows.len() != 6 || faces.len() != 6 {
        return Err(invalid(
            "Six distinct orientations are required, each with a stable data window",
        ));
    }
    let mut m = vec![vec![0.0; 7]; 6];
    for (row, w) in windows.iter().enumerate() {
        for i in 0..3 {
            m[row][i] = w.accel_g[i].powi(2);
            m[row][i + 3] = w.accel_g[i];
        }
        m[row][6] = 1.0;
    }
    for col in 0..6 {
        let pivot = (col..6)
            .max_by(|a, b| m[*a][col].abs().partial_cmp(&m[*b][col].abs()).unwrap())
            .unwrap();
        if m[pivot][col].abs() < 1e-7 {
            return Err(invalid(
                "Six-face data does not support a stable fit; collect the faces again",
            ));
        }
        m.swap(col, pivot);
        let scale = m[col][col];
        for v in &mut m[col] {
            *v /= scale;
        }
        for row in 0..6 {
            if row != col {
                let factor = m[row][col];
                let pivot_row = m[col].clone();
                for (value, pivot_value) in m[row].iter_mut().zip(pivot_row) {
                    *value -= factor * pivot_value;
                }
            }
        }
    }
    let q: Vec<f64> = m.iter().map(|row| row[6]).collect();
    if q[..3].iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return Err(invalid(
            "Calibration fit is invalid; check the six-face recordings",
        ));
    }
    let bias: Vec<f64> = (0..3).map(|i| -q[i + 3] / (2.0 * q[i])).collect();
    let denominator = 1.0 + (0..3).map(|i| q[i] * bias[i] * bias[i]).sum::<f64>();
    previous.accel_bias_ug = bias
        .iter()
        .map(|v| (v * 1e6).round_ties_even() as i32)
        .collect();
    previous.accel_scale_ppm = (0..3)
        .map(|i| ((q[i] / denominator).sqrt() * 1e6).round_ties_even() as i32)
        .collect();
    check_calibration(&previous)?;
    Ok(previous)
}
#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum CalibrationComponent {
    All,
    Accel,
    Gyro,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct ImuValidation {
    pub max_gravity_error_g: f64,
    pub max_gyro_residual_dps: f64,
}
#[uniffi::export]
pub fn validate_imu_calibration(
    windows: Vec<ImuStaticWindow>,
    full: bool,
    component: CalibrationComponent,
) -> Result<ImuValidation> {
    if windows.len() < if full { 3 } else { 1 } {
        return Err(invalid("Not enough independent validation data"));
    }
    for w in &windows {
        check_window(w)?;
    }
    if full {
        let faces: Vec<_> = windows
            .iter()
            .map(|w| imu_window_face(w.clone()))
            .collect::<Result<_>>()?;
        if faces
            .iter()
            .map(|f| f.as_bytes()[0])
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            < 3
        {
            return Err(invalid("Validation must cover all three axes"));
        }
    }
    let gravity = windows
        .iter()
        .map(|w| (norm(&w.accel_g) - 1.0).abs())
        .fold(0.0, f64::max);
    let gyro = windows
        .iter()
        .flat_map(|w| &w.gyro_dps)
        .map(|v| v.abs())
        .fold(0.0, f64::max);
    if (!matches!(component, CalibrationComponent::Gyro)
        && (full || matches!(component, CalibrationComponent::Accel))
        && gravity > 0.015)
        || (!matches!(component, CalibrationComponent::Accel) && gyro > 0.15)
    {
        return Err(invalid(format!("Validation failed: gravity magnitude error {gravity:.4} g, gyro residual {gyro:.3} deg/s")));
    }
    Ok(ImuValidation {
        max_gravity_error_g: gravity,
        max_gyro_residual_dps: gyro,
    })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ImuCollectionPhase {
    Ready,
    Collecting,
    Complete,
    Failed,
    Cancelled,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct ImuCollectionProgress {
    pub phase: ImuCollectionPhase,
    pub collected: u32,
    pub target: u32,
    pub settling_remaining: u32,
}
struct StaticCollection {
    phase: ImuCollectionPhase,
    settling_remaining: u32,
    next_sequence: Option<u32>,
    samples: Vec<ImuSample>,
    window: Option<ImuStaticWindow>,
    now: u64,
    deadline: u64,
}
#[derive(uniffi::Object)]
pub struct ImuStaticCollector {
    state: Mutex<StaticCollection>,
    config: ImuConfig,
    target: u32,
    timeout_ms: u64,
    check_gyro: bool,
}
#[uniffi::export]
impl ImuStaticCollector {
    #[uniffi::constructor]
    pub fn new(
        config: ImuConfig,
        duration_ms: u32,
        settle_ms: u32,
        check_gyro: bool,
    ) -> Result<Self> {
        if !(2000..=120000).contains(&duration_ms)
            || settle_ms > 30000
            || !(1..=1000).contains(&config.sample_rate_hz)
        {
            return Err(invalid("Invalid static IMU sampling duration or rate"));
        }
        identity_imu_calibration(config.clone())?;
        let count = |duration| {
            (u64::from(duration) * u64::from(config.sample_rate_hz)).div_ceil(1000) as u32
        };
        let target = count(duration_ms);
        Ok(Self {
            state: Mutex::new(StaticCollection {
                phase: ImuCollectionPhase::Ready,
                settling_remaining: count(settle_ms),
                next_sequence: None,
                samples: Vec::with_capacity(target as usize),
                window: None,
                now: 0,
                deadline: 0,
            }),
            config,
            target,
            timeout_ms: u64::from(duration_ms) + u64::from(settle_ms) + 10000,
            check_gyro,
        })
    }
    pub fn begin(&self, now_ms: u64) -> Result<ImuCollectionProgress> {
        let mut state = self.state.lock().unwrap();
        if state.phase != ImuCollectionPhase::Ready {
            return Err(invalid("Static IMU collection already started"));
        }
        let Some(deadline) = now_ms.checked_add(self.timeout_ms) else {
            state.phase = ImuCollectionPhase::Failed;
            return Err(invalid("Static IMU collection deadline overflow"));
        };
        state.now = now_ms;
        state.deadline = deadline;
        state.phase = ImuCollectionPhase::Collecting;
        Ok(self.progress(&state))
    }
    pub fn accept(&self, batch: ImuBatch, now_ms: u64) -> Result<ImuCollectionProgress> {
        let mut state = self.state.lock().unwrap();
        if state.phase != ImuCollectionPhase::Collecting {
            return Err(invalid("Static IMU collection is not active"));
        }
        let result = (|| {
            Self::check_time(&mut state, now_ms)?;
            if batch.frame_count == 0
                || batch.frame_count as usize != batch.samples.len()
                || batch.sample_size != 16
                || state
                    .next_sequence
                    .is_some_and(|next| next != batch.sequence_start)
            {
                return Err(invalid("IMU sample sequence gap or invalid batch"));
            }
            state.next_sequence = Some(
                batch
                    .sequence_start
                    .wrapping_add(u32::from(batch.frame_count)),
            );
            for sample in batch.samples {
                if state.settling_remaining > 0 {
                    state.settling_remaining -= 1;
                } else if state.samples.len() < self.target as usize {
                    state.samples.push(sample);
                }
            }
            if state.samples.len() == self.target as usize {
                state.window = Some(analyze_imu_static_window(
                    std::mem::take(&mut state.samples),
                    self.config.clone(),
                    self.check_gyro,
                )?);
                state.phase = ImuCollectionPhase::Complete;
            }
            Ok(self.progress(&state))
        })();
        if result.is_err() {
            state.phase = ImuCollectionPhase::Failed;
            state.samples.clear();
        }
        result
    }
    pub fn tick(&self, now_ms: u64) -> Result<ImuCollectionProgress> {
        let mut state = self.state.lock().unwrap();
        if state.phase != ImuCollectionPhase::Collecting {
            return Err(invalid("Static IMU collection is not active"));
        }
        if let Err(error) = Self::check_time(&mut state, now_ms) {
            state.phase = ImuCollectionPhase::Failed;
            state.samples.clear();
            return Err(error);
        }
        Ok(self.progress(&state))
    }
    pub fn next_deadline(&self) -> Option<u64> {
        let state = self.state.lock().unwrap();
        (state.phase == ImuCollectionPhase::Collecting).then_some(state.deadline)
    }
    pub fn finish(&self) -> Result<ImuStaticWindow> {
        let state = self.state.lock().unwrap();
        if state.phase != ImuCollectionPhase::Complete {
            return Err(invalid("Static IMU collection is not complete"));
        }
        Ok(state.window.as_ref().unwrap().clone())
    }
    pub fn cancel(&self) {
        let mut state = self.state.lock().unwrap();
        state.phase = ImuCollectionPhase::Cancelled;
        state.samples.clear();
        state.window = None;
    }
}
impl ImuStaticCollector {
    fn progress(&self, state: &StaticCollection) -> ImuCollectionProgress {
        ImuCollectionProgress {
            phase: state.phase,
            collected: state
                .window
                .as_ref()
                .map_or(state.samples.len() as u32, |window| window.samples),
            target: self.target,
            settling_remaining: state.settling_remaining,
        }
    }
    fn check_time(state: &mut StaticCollection, now_ms: u64) -> Result<()> {
        if now_ms < state.now {
            return Err(invalid("Monotonic clock moved backwards"));
        }
        state.now = now_ms;
        if now_ms >= state.deadline {
            return Err(CoreError::Timeout);
        }
        Ok(())
    }
}
#[derive(Default)]
struct Continuity {
    next: Option<u32>,
    timestamp: Option<u32>,
    elapsed: u64,
    failed: bool,
}
#[derive(uniffi::Object, Default)]
pub struct ImuContinuity {
    state: Mutex<Continuity>,
}
#[uniffi::export]
impl ImuContinuity {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self::default()
    }
    /// Returns unwrapped device timestamps. Reset by creating a new object per connection.
    pub fn accept(&self, batch: ImuBatch) -> Result<Vec<u64>> {
        let mut s = self.state.lock().unwrap();
        let result = (|| {
            if s.failed
                || batch.frame_count == 0
                || batch.frame_count as usize != batch.samples.len()
                || batch.sample_size != 16
                || s.next.is_some_and(|v| v != batch.sequence_start)
            {
                return Err(invalid("IMU sample sequence gap or invalid batch"));
            }
            let mut times = Vec::new();
            for sample in &batch.samples {
                match s.timestamp {
                    None => s.elapsed = sample.timestamp_ms as u64,
                    Some(previous) => {
                        let delta = sample.timestamp_ms.wrapping_sub(previous);
                        if delta == 0 || delta >= 0x80000000 {
                            return Err(invalid("IMU timestamp is repeated or moves backwards"));
                        }
                        s.elapsed = s
                            .elapsed
                            .checked_add(delta as u64)
                            .ok_or_else(|| invalid("IMU timestamp overflow"))?;
                    }
                }
                s.timestamp = Some(sample.timestamp_ms);
                times.push(s.elapsed);
            }
            s.next = Some(batch.sequence_start.wrapping_add(batch.frame_count as u32));
            Ok(times)
        })();
        if result.is_err() {
            s.failed = true;
        }
        result
    }
}
