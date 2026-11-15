// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb::{map, set};
use super::super::by_name::by_name;
use super::super::consts::METADATA_TAG;
use super::super::metadata_port_mac::metadata_port_mac;
use super::super::metadata_port_name::metadata_port_name;
use super::super::onto_switch::onto_switch;
use super::super::ovn_backend::OvnBackend;
use super::super::provnet_name::provnet_name;
use super::super::pub_consts::METADATA_ADDRESS;
use super::super::switch_name::switch_name;
use crate::types::{Network, NetworkKind};
use pistack_types::ApiResult;
use serde_json::json;

impl OvnBackend {
    /// `Logical_Switch`:
    /// * `name = ps-<net id>`
    /// * `other_config:mtu = <spec.mtu>`
    /// * Overlay: nothing else; OVN picks the Geneve tunnel key.
    /// * Provider: a `Logical_Switch_Port` named `provnet-<net id>` with
    ///   `type=localnet`, `options:network_name=<physnet>`,
    ///   `addresses=["unknown"]`, and `tag=<vlan>` when set.
    /// * `external` networks additionally get
    ///   `external_ids:pistack:external=true` so routers can find them.
    /// * `other_config:vlan-passthru` stays unset (no Q-in-Q).
    /// * Every network: a `Logical_Switch_Port` named `pimeta-<net uuid>`
    ///   with `type=localport`,
    ///   `addresses=["<stable 06: mac> 169.254.169.254"]` and
    ///   `external_ids:pistack:metadata=true`.
    pub(in crate::backend::ovn::ovs_net_backend) async fn ensure_network_body(
        &self,
        net: &Network,
    ) -> ApiResult<()> {
        let id = &net.meta.id;
        let name = switch_name(id);
        let mut ext = self.tags(id, net.meta.project.as_ref());
        if net.spec.external {
            ext.push(("pistack:external".into(), "true".into()));
        }
        let row = json!({
            "name": name,
            "other_config": map([("mtu", net.spec.mtu.to_string())]),
            "external_ids": map(ext.clone()),
        });
        self.ensure_row("Logical_Switch", by_name(&name), row, vec![])
            .await?;
        if let NetworkKind::Provider { vlan, physnet } = &net.spec.kind {
            let uplink = provnet_name(id);
            let tag = vlan.map(|v| set([json!(v)])).unwrap_or_else(|| set([]));
            let row = json!({
                "name": uplink,
                "type": "localnet",
                "addresses": "unknown",
                "options": map([("network_name", physnet.as_str())]),
                "tag": tag,
                "external_ids": map(ext.clone()),
            });
            self.ensure_row(
                "Logical_Switch_Port",
                by_name(&uplink),
                row,
                onto_switch(&name),
            )
            .await?;
        }
        let meta = metadata_port_name(id);
        ext.push((METADATA_TAG.into(), "true".into()));
        let row = json!({
            "name": meta,
            "type": "localport",
            "addresses": format!("{} {METADATA_ADDRESS}", metadata_port_mac(id)),
            "external_ids": map(ext),
        });
        self.ensure_row(
            "Logical_Switch_Port",
            by_name(&meta),
            row,
            onto_switch(&name),
        )
        .await?;
        Ok(())
    }
}
