// SPDX-License-Identifier: MPL-2.0
use crate::{invalid, CoreError, Result};
use std::sync::Mutex;

struct State {
    last: u64,
    total: u64,
    idle: u64,
    failed: bool,
}

/// Shared idle/total timeout policy; the platform supplies a monotonic clock.
#[derive(uniffi::Object)]
pub struct DeadlineBudget {
    state: Mutex<State>,
    idle_timeout_ms: u64,
}

#[uniffi::export]
impl DeadlineBudget {
    #[uniffi::constructor]
    pub fn new(now_ms: u64, idle_timeout_ms: u64, total_timeout_ms: u64) -> Result<Self> {
        if idle_timeout_ms == 0 || total_timeout_ms == 0 {
            return Err(invalid("Timeouts must be positive"));
        }
        Ok(Self {
            state: Mutex::new(State {
                last: now_ms,
                total: now_ms
                    .checked_add(total_timeout_ms)
                    .ok_or_else(|| invalid("Total deadline overflow"))?,
                idle: now_ms
                    .checked_add(idle_timeout_ms)
                    .ok_or_else(|| invalid("Idle deadline overflow"))?,
                failed: false,
            }),
            idle_timeout_ms,
        })
    }
    pub fn remaining(&self, now_ms: u64) -> Result<u64> {
        let mut s = self.state.lock().unwrap();
        check(&mut s, now_ms)?;
        Ok(s.total.min(s.idle) - now_ms)
    }
    /// Only actual I/O progress refreshes idle time; total time is never extended.
    pub fn progress(&self, now_ms: u64) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        check(&mut s, now_ms)?;
        s.idle = now_ms.saturating_add(self.idle_timeout_ms);
        Ok(())
    }
    pub fn next_deadline(&self) -> u64 {
        let s = self.state.lock().unwrap();
        s.total.min(s.idle)
    }
}

fn check(s: &mut State, now: u64) -> Result<()> {
    if s.failed {
        return Err(CoreError::Timeout);
    }
    if now < s.last {
        s.failed = true;
        return Err(invalid("Monotonic clock moved backwards"));
    }
    s.last = now;
    if now >= s.total || now >= s.idle {
        s.failed = true;
        return Err(CoreError::Timeout);
    }
    Ok(())
}
