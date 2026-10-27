// SPDX-License-Identifier: MPL-2.0
use crate::{invalid, Result};
use std::sync::Mutex;
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DistanceQuality {
    Unknown,
    Low,
    Medium,
    High,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DistanceSource {
    AndroidBleCs,
    Replay,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DistanceState {
    Waiting,
    Measuring,
    LowQuality,
    Stale,
    Unavailable,
    Disconnected,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct DistanceObservation {
    pub meters: f64,
    pub quality: DistanceQuality,
    pub source: DistanceSource,
    pub measured_at_ms: u64,
    pub generation: u64,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct DistanceReading {
    pub state: DistanceState,
    pub observation: Option<DistanceObservation>,
}
struct State {
    generation: u64,
    available: bool,
    connected: bool,
    last: Option<DistanceObservation>,
    now: u64,
}
#[derive(uniffi::Object)]
pub struct DistanceTracker {
    state: Mutex<State>,
    max_age_ms: u64,
    max_meters: f64,
}
#[uniffi::export]
impl DistanceTracker {
    #[uniffi::constructor]
    pub fn new(max_age_ms: u64, max_meters: f64) -> Result<Self> {
        if max_age_ms == 0 || !max_meters.is_finite() || max_meters <= 0.0 {
            return Err(invalid("Invalid distance limits"));
        }
        Ok(Self {
            state: Mutex::new(State {
                generation: 0,
                available: false,
                connected: false,
                last: None,
                now: 0,
            }),
            max_age_ms,
            max_meters,
        })
    }
    pub fn open(&self, generation: u64, available: bool, now_ms: u64) {
        *self.state.lock().unwrap() = State {
            generation,
            available,
            connected: true,
            last: None,
            now: now_ms,
        };
    }
    pub fn accept(&self, observation: DistanceObservation, now_ms: u64) -> Result<DistanceReading> {
        let mut s = self.state.lock().unwrap();
        check_clock(&mut s, now_ms)?;
        if observation.generation != s.generation || !s.connected || !s.available {
            return Ok(reading(&mut s, now_ms, self.max_age_ms));
        }
        if !observation.meters.is_finite()
            || observation.meters < 0.0
            || observation.meters > self.max_meters
            || observation.measured_at_ms > now_ms
        {
            return Err(invalid("Invalid distance or measurement timestamp"));
        }
        // Out-of-order observations never replace a newer measurement.
        if !s
            .last
            .as_ref()
            .is_some_and(|v| observation.measured_at_ms <= v.measured_at_ms)
            && now_ms - observation.measured_at_ms <= self.max_age_ms
        {
            s.last = Some(observation);
        }
        Ok(reading(&mut s, now_ms, self.max_age_ms))
    }
    pub fn tick(&self, now_ms: u64) -> Result<DistanceReading> {
        let mut s = self.state.lock().unwrap();
        check_clock(&mut s, now_ms)?;
        Ok(reading(&mut s, now_ms, self.max_age_ms))
    }
    pub fn disconnect(&self, generation: u64) {
        let mut s = self.state.lock().unwrap();
        if s.generation == generation {
            s.connected = false;
            s.last = None;
        }
    }
}
fn check_clock(s: &mut State, now: u64) -> Result<()> {
    if now < s.now {
        return Err(invalid("Monotonic clock moved backwards"));
    }
    s.now = now;
    Ok(())
}
fn reading(s: &mut State, now: u64, max_age: u64) -> DistanceReading {
    let state = if !s.connected {
        DistanceState::Disconnected
    } else if !s.available {
        DistanceState::Unavailable
    } else if let Some(v) = &s.last {
        if now.saturating_sub(v.measured_at_ms) > max_age {
            DistanceState::Stale
        } else if matches!(v.quality, DistanceQuality::Low | DistanceQuality::Unknown) {
            DistanceState::LowQuality
        } else {
            DistanceState::Measuring
        }
    } else {
        DistanceState::Waiting
    };
    let observation = if matches!(state, DistanceState::Measuring | DistanceState::LowQuality) {
        s.last.clone()
    } else {
        None
    };
    DistanceReading { state, observation }
}
