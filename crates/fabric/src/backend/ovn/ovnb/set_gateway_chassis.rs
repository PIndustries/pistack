// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb;
use super::super::super::ovsdb::{eq, map, set, uuid};
use super::super::pub_consts::GATEWAY_CHASSIS_ENV;
use super::super::{ovn_backend::OvnBackend, uuid_strings::uuid_strings};
use pistack_types::ApiResult;
use serde_json::json;

impl OvnBackend {
    /// Point the gateway port at one `Gateway_Chassis` row per configured
    /// chassis. Rewrites the rows only when the chassis or priorities differ.
    pub(in crate::backend::ovn) async fn set_gateway_chassis(
        &self,
        rtr: &str,
        lrp: &str,
        lrp_uuid: &str,
    ) -> ApiResult<()> {
        let chassis = self.gateway_chassis()?;
        if chassis.is_empty() {
            tracing::warn!(
            router = rtr,
            "ovn: no chassis can host the gateway port, so the router has no way out. Set {GATEWAY_CHASSIS_ENV}, or {} on an agent whose host has the uplink",
            "PISTACK_OVN_BRIDGE_MAPPINGS"
        );
        }
        let n = chassis.len();
        let mut want: Vec<(String, i64)> = chassis
            .iter()
            .enumerate()
            .map(|(i, c)| (c.clone(), (n - i) as i64))
            .collect();
        let at = vec![eq("_uuid", uuid(lrp_uuid))];
        let found = self
            .run(vec![ovsdb::select("Logical_Router_Port", at.clone())])
            .await?;
        let held = uuid_strings(
            ovsdb::rows(&found[0])
                .first()
                .and_then(|r| r.get("gateway_chassis")),
        );
        let old = if held.is_empty() {
            Vec::new()
        } else {
            let ops = held
                .iter()
                .map(|u| ovsdb::select("Gateway_Chassis", vec![eq("_uuid", uuid(u))]))
                .collect();
            let out = self.run(ops).await?;
            out.iter().flat_map(ovsdb::rows).collect()
        };
        let mut have: Vec<(String, i64)> = old
            .iter()
            .map(|r| {
                (
                    r["chassis_name"].as_str().unwrap_or("").to_string(),
                    r["priority"].as_i64().unwrap_or(0),
                )
            })
            .collect();
        have.sort();
        let order = want.clone();
        want.sort();
        if have == want {
            return Ok(());
        }
        let mut ops = Vec::new();
        let mut names = Vec::new();
        for (i, (chassis, priority)) in order.iter().enumerate() {
            let n = format!("gc{i}");
            ops.push(ovsdb::insert(
                "Gateway_Chassis",
                json!({
                    "name": format!("{lrp}_{chassis}"),
                    "chassis_name": chassis,
                    "priority": priority,
                    "external_ids": map([
                        ("pistack:id".to_string(), rtr.to_string()),
                        ("pistack:term".to_string(), self.term().to_string()),
                    ]),
                }),
                &n,
            ));
            names.push(ovsdb::named(&n));
        }
        ops.push(ovsdb::update(
            "Logical_Router_Port",
            at,
            json!({ "gateway_chassis": set(names) }),
        ));
        for u in &held {
            ops.push(ovsdb::delete("Gateway_Chassis", vec![eq("_uuid", uuid(u))]));
        }
        self.run(ops).await?;
        Ok(())
    }
}
