// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::ovn_backend::OvnBackend;
use super::super::tagged::tagged;
use pistack_types::{ApiResult, Id};

impl OvnBackend {
    /// Take the port off its switch and delete it. A second delete
    /// succeeds. `Port_Group.ports` refers to switch ports weakly, so the
    /// port leaves its groups with it.
    pub(in crate::backend::ovn::ovs_net_backend) async fn delete_port_body(
        &self,
        id: &Id,
    ) -> ApiResult<()> {
        self.drop_switch_ports(tagged("pistack:id", id)).await
    }
}
