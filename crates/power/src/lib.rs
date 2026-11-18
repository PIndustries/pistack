// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Out-of-band power control over a baseboard management controller.
//!
//! Two transports, reached through the same [`PowerControl`] trait so a
//! caller can be tested against a fake:
//!
//! * `ipmi` drives `ipmitool` over `lanplus`.
//! * `redfish` drives a Redfish service with `curl`.
//!
//! The password is a [`Secret`]: an [`Ipmitool`] or [`Redfish`] reads it
//! from a file (a path, never the bytes), while a caller that holds the
//! password in memory hands over a [`Secret::Bytes`]. Either way the
//! password is kept out of the environment, the command line and every log
//! line: `ipmitool` is given a file it reads itself, and `curl` a config
//! file written for the one request and removed after it.

mod bmc_host;
mod bmc_user;
mod chassis_power;
mod clip;
mod ipmitool;
mod kind;
mod nanoid;
mod pass_file;
mod power_control;
mod redfish;
mod run;
mod secret;
mod spawn_retry;
mod tool;
mod which;

#[cfg(test)]
mod tests;

pub use chassis_power::ChassisPower;
pub use ipmitool::Ipmitool;
pub use kind::Kind;
pub use power_control::PowerControl;
pub use redfish::Redfish;
pub use secret::Secret;
