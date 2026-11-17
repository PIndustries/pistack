// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Alarm resources and the sample point posted to gauge.

mod alarm;
mod alarm_phase;
mod alarm_spec;
mod alarm_spec_update;
mod alarm_status;
mod comparison;
mod eval;
mod sample;
#[cfg(test)]
mod tests;
mod window;

pub use alarm::Alarm;
pub use alarm_phase::AlarmPhase;
pub use alarm_spec::AlarmSpec;
pub use alarm_spec_update::AlarmSpecUpdate;
pub use alarm_status::AlarmStatus;
pub use comparison::Comparison;
pub use sample::Sample;
pub use window::{ALARM_REFRESH, MAX_HOLD, MAX_SERIES, STALE_AFTER, WINDOW_AGE, WINDOW_POINTS};

pub(crate) use eval::{crosses, decide_on, hold_coverable, phase_for};
