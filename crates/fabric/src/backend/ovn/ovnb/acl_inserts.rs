// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb;
use super::super::super::ovsdb::map;
use super::super::{acl::Acl, ovn_backend::OvnBackend};
use serde_json::{json, Value};

impl OvnBackend {
    /// Insert ops for `acls` tagged with `group`; returns their named refs.
    pub(in crate::backend::ovn) fn acl_inserts(
        &self,
        group: &str,
        acls: &[Acl],
        ops: &mut Vec<Value>,
    ) -> Vec<Value> {
        let term = self.term().to_string();
        acls.iter()
            .enumerate()
            .map(|(i, a)| {
                let name = format!("acl{i}");
                let mut ext = vec![
                    ("pistack:group".to_string(), group.to_string()),
                    ("pistack:term".to_string(), term.clone()),
                ];
                if let Some(r) = &a.rule {
                    ext.push(("pistack:rule".into(), r.clone()));
                }
                let row = json!({
                    "direction": a.direction,
                    "priority": a.priority,
                    "match": a.matches,
                    "action": a.action,
                    "external_ids": map(ext),
                });
                ops.push(ovsdb::insert("ACL", row, &name));
                ovsdb::named(&name)
            })
            .collect()
    }
}
