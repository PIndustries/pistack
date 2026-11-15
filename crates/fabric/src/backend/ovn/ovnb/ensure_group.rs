// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use serde_json::Value;

use super::super::super::ovsdb;
use super::super::super::ovsdb::{eq, includes, map, set, uuid};
use super::super::{
    acl::Acl, by_name::by_name, ovn_backend::OvnBackend, same_acls::same_acls,
    uuid_strings::uuid_strings,
};
use pistack_types::{ApiError, ApiResult};
use serde_json::json;

impl OvnBackend {
    /// Make `Port_Group` `name` exist with `ext`. `acls` and `ports`, when
    /// given, replace what the group has if it differs; `None` leaves them
    /// alone (empty on a new group). Replaced ACLs are dropped by the
    /// database once nothing refers to them.
    pub(in crate::backend::ovn) async fn ensure_group(
        &self,
        name: &str,
        ext: Vec<(String, String)>,
        acls: Option<&[Acl]>,
        ports: Option<Vec<String>>,
    ) -> ApiResult<()> {
        let found = self
            .run(vec![
                ovsdb::select("Port_Group", by_name(name)),
                ovsdb::select(
                    "ACL",
                    vec![includes("external_ids", map([("pistack:group", name)]))],
                ),
            ])
            .await?;
        let groups = ovsdb::rows(&found[0]);
        let have_acls = ovsdb::rows(&found[1]);
        let mut ops = Vec::new();
        match groups.as_slice() {
            [] => {
                ops.push(ovsdb::absent("Port_Group", by_name(name)));
                let refs = acls
                    .map(|a| self.acl_inserts(name, a, &mut ops))
                    .unwrap_or_default();
                let ports = ports.unwrap_or_default();
                ops.push(ovsdb::insert(
                    "Port_Group",
                    json!({
                        "name": name,
                        "acls": set(refs),
                        "ports": set(ports.iter().map(|u| uuid(u))),
                        "external_ids": map(ext),
                    }),
                    "pg",
                ));
                self.run(ops).await?;
                Ok(())
            }
            [one] => {
                let u = ovsdb::row_uuid(one)
                    .ok_or_else(|| ApiError::unavailable("Port_Group row: no uuid"))?;
                let mut row = serde_json::Map::new();
                if let Some(want) = acls {
                    if !same_acls(&have_acls, want) {
                        let refs = self.acl_inserts(name, want, &mut ops);
                        row.insert("acls".into(), set(refs));
                    }
                }
                if let Some(mut want) = ports {
                    want.sort();
                    want.dedup();
                    if uuid_strings(one.get("ports")) != want {
                        row.insert("ports".into(), set(want.iter().map(|u| uuid(u))));
                    }
                }
                let ext = map(ext);
                if !ovsdb::row_matches(one, &json!({ "external_ids": ext })) {
                    row.insert("external_ids".into(), ext);
                }
                if !row.is_empty() {
                    ops.push(ovsdb::update(
                        "Port_Group",
                        vec![eq("_uuid", uuid(&u))],
                        Value::Object(row),
                    ));
                    self.run(ops).await?;
                }
                Ok(())
            }
            many => Err(ApiError::conflict(format!(
                "{} Port_Group rows named {name}",
                many.len()
            ))),
        }
    }
}
