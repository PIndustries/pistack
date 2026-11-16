// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb;
use super::super::ovn_backend::OvnBackend;
use super::super::tagged::tagged;
use super::super::unhook_nat::unhook_nat;
use pistack_types::{ApiResult, Id};

impl OvnBackend {
    /// Remove any `NAT` row with `external_ids:pistack:id = <fip id>`.
    pub(in crate::backend::ovn::ovs_net_backend) async fn delete_fip_body(
        &self,
        id: &Id,
    ) -> ApiResult<()> {
        let found = self
            .run(vec![ovsdb::select("NAT", tagged("pistack:id", id))])
            .await?;
        let ops = unhook_nat(&ovsdb::rows(&found[0]));
        if !ops.is_empty() {
            self.run(ops).await?;
        }
        Ok(())
    }
}
