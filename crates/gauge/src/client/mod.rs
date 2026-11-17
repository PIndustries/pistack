// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Writes samples and alarms. The only place alarm `status` is updated.

mod choice;
mod client_type;
mod format_value;
mod stat;
#[cfg(test)]
mod tests;
mod validate_alarm;
mod validate_name;

pub use client_type::Client;
pub use stat::Stat;
