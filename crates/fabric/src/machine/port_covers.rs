// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::FloatingIp;
use pistack_types::Id;
use std::net::IpAddr;

/// A floating IP already serves this machine when it points at a port whose
/// device is the machine, or whose fixed IP is the guest address.
pub(crate) fn port_covers(
    fip: &FloatingIp,
    ports: &[crate::types::Port],
    machine: &Id,
    addr: IpAddr,
) -> bool {
    let Some(pid) = &fip.spec.port else {
        return false;
    };
    let Some(port) = ports.iter().find(|p| &p.meta.id == pid) else {
        return false;
    };
    port.spec.device.as_ref() == Some(machine) || port.spec.fixed_ips.contains(&addr)
}
