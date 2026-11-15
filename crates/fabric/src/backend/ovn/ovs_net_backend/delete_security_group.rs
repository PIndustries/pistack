// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb;
use super::super::by_name::by_name;
use super::super::group_name::group_name;
use super::super::ovn_backend::OvnBackend;
use pistack_types::{ApiResult, Id};

impl OvnBackend {
    /// Delete `pg_<sg uuid>`; the database drops its ACLs with it.
    pub(in crate::backend::ovn::ovs_net_backend) async fn delete_security_group_body(
        &self,
        id: &Id,
    ) -> ApiResult<()> {
        self.run(vec![ovsdb::delete("Port_Group", by_name(&group_name(id)))])
            .await?;
        Ok(())
    }
}
