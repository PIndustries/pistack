// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiResult, Id};
impl Client {
    /// Point the floating IP at a machine's current IPv4.
    ///
    /// A port is created on the longest-prefix subnet that contains that
    /// address, in this floating IP's project. A later call reuses that port.
    pub fn associate_to_machine(&self, id: &Id, machine: &Id) -> ApiResult<FloatingIp> {
        let fip = self.floating_ips().get(id).map_err(store_err)?;
        let guest = crate::machine::guest_ipv4(&self.store, machine)?;
        check_same_project(&fip.meta, &guest.meta, "machine")?;
        let port = self.ensure_machine_port(&fip.meta, machine, guest.ipv4, &guest.meta.name)?;
        self.associate_floating_ip(id, Some(port))
    }
}
