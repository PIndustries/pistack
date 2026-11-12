// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! What forge needs to know when the cluster's networks are OVN.
//!
//! Fabric writes the logical network to OVN. A machine's port only works on
//! a host that is an OVN chassis: Open vSwitch pointed at the southbound
//! database, the integration bridge present, and `ovn-controller` connected.
//! The agent reports that in each heartbeat and the registrar keeps the
//! last report in `HostStatus::ovn`.

use crate::types::{usable_underlay, Host};

/// True when this process realises tenant networks through OVN.
pub fn enabled() -> bool {
    std::env::var("PISTACK_FABRIC_BACKEND").ok().as_deref() == Some("ovn")
}

/// True when the host's last heartbeat said it is a working chassis.
pub(crate) fn chassis_ready(host: &Host) -> bool {
    host.status.ovn.as_ref().is_some_and(|o| o.ready)
}

/// Why `host` cannot take a machine on an OVN network. `None` when it can.
pub(crate) fn not_usable(host: &Host) -> Option<Refusal> {
    if usable_underlay(host.spec.underlay_ip.as_deref()).is_none() {
        return Some(Refusal::NoUnderlay);
    }
    (!chassis_ready(host)).then_some(Refusal::NotReady)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// No tunnel endpoint.
    NoUnderlay,
    /// No report yet, or the report says the chassis is not working.
    NotReady,
}
