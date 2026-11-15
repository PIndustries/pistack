// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb;
use super::super::by_name::by_name;
use super::super::ovn_backend::OvnBackend;
use super::super::switch_name::switch_name;
use super::super::tagged::tagged;
use pistack_types::{ApiResult, Id};

impl OvnBackend {
    /// Delete the `Logical_Switch`; OVN drops its ports with it. Also
    /// removes the network's `DHCP_Options` rows. A second delete succeeds.
    pub(in crate::backend::ovn::ovs_net_backend) async fn delete_network_body(
        &self,
        id: &Id,
    ) -> ApiResult<()> {
        self.run(vec![
            ovsdb::delete("Logical_Switch", by_name(&switch_name(id))),
            ovsdb::delete("DHCP_Options", tagged("pistack:network", id)),
        ])
        .await?;
        Ok(())
    }
}
