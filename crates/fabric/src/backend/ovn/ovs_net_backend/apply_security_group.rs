// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb;
use super::super::by_name::by_name;
use super::super::lsp_name::lsp_name;
use super::super::ovn_backend::OvnBackend;
use crate::types::{Port, SecurityGroup};
use pistack_types::ApiResult;

impl OvnBackend {
    /// One `Port_Group` per security group:
    /// * `name = pg_<sg uuid without dashes>`, `ports = {member switch
    ///   ports with port security}`, `external_ids:pistack:id = <sg id>`.
    /// * OVN derives address sets `pg_<uuid>_ip4` / `_ip6` from the
    ///   group's ports; `Remote::Group` rules match on those. A group a
    ///   rule names is created empty if it does not exist yet.
    /// * Per rule an `ACL` in `Port_Group.acls`: `direction = to-lport`
    ///   (ingress) or `from-lport` (egress), `priority = 1002`,
    ///   `action = allow-related`, and a match such as
    ///   `outport == @pg_<uuid> && ip4 && ip4.src == 10.0.0.0/8 && tcp && tcp.dst == 22`
    ///   (`inport` and `ip4.dst` for egress, `ip6` for IPv6, `icmp4` /
    ///   `icmp6` for ICMP; no address term for `Remote::Any`, no L4 term
    ///   for any protocol). A rule whose remote address is of the other
    ///   family matches nothing and gets no ACL.
    /// * Default deny lives in one shared group, `pg_pistack_drop`, that
    ///   every port with port security joins: drop all IP both ways at
    ///   `priority = 1001`, allow DHCP and DHCPv6 requests out at 1002.
    /// * ACLs carry `external_ids:pistack:group = <group name>` and
    ///   `pistack:rule = <rule id>`. When the rules change, the group's
    ///   ACL set is replaced in one transaction; unchanged rules write
    ///   nothing.
    pub(in crate::backend::ovn::ovs_net_backend) async fn apply_security_group_body(
        &self,
        sg: &SecurityGroup,
        members: &[Port],
    ) -> ApiResult<()> {
        let names: Vec<String> = members
            .iter()
            .filter(|p| {
                p.spec.port_security && !p.spec.mac.trim().is_empty() && !p.is_router_port()
            })
            .map(|p| lsp_name(&p.meta.id))
            .collect();
        let ports = if names.is_empty() {
            Vec::new()
        } else {
            let ops = names
                .iter()
                .map(|n| ovsdb::select("Logical_Switch_Port", by_name(n)))
                .collect();
            let out = self.run(ops).await?;
            out.iter()
                .flat_map(ovsdb::rows)
                .filter_map(|r| ovsdb::row_uuid(&r))
                .collect()
        };
        self.ensure_sg_group(sg, Some(ports)).await
    }
}
