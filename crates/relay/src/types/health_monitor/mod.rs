// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The active health check a pool runs: its kind and its settings. Absent
//! on a pool means its members are `NoMonitor`.

mod monitor_type;
mod settings;

pub use monitor_type::MonitorType;
pub use settings::HealthMonitor;
