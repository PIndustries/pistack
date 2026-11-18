// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! What forge needs to know when the cluster's networks are OVN.
//!
//! Fabric writes the logical network to OVN. A machine's port only works on
//! a host that is an OVN chassis: Open vSwitch pointed at the southbound
//! database, the integration bridge present, and `ovn-controller` connected.
//! The agent reports that in each heartbeat and the registrar keeps the
//! last report in `HostStatus::ovn`.

mod chassis_ready;
mod enabled;
mod not_usable;
mod refusal;
#[cfg(test)]
pub(crate) use chassis_ready::chassis_ready;
pub use enabled::enabled;
pub(crate) use not_usable::not_usable;
pub(crate) use refusal::Refusal;
