// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb;
use super::super::ovn_backend::OvnBackend;
use super::super::tagged::tagged;
use pistack_types::{ApiResult, Id};

impl OvnBackend {
    /// Delete the subnet's `DHCP_Options` row. Ports referring to it lose
    /// the reference with it.
    pub(in crate::backend::ovn::ovs_net_backend) async fn delete_subnet_body(
        &self,
        id: &Id,
    ) -> ApiResult<()> {
        self.run(vec![ovsdb::delete(
            "DHCP_Options",
            tagged("pistack:subnet", id),
        )])
        .await?;
        Ok(())
    }
}
