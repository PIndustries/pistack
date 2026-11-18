// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::{normalize_name, RecordType, Zone, MANAGED_LABEL};
use pistack_types::{ApiResult, Meta};
use std::collections::BTreeMap;
use std::net::IpAddr;

use super::owner::OWNER;

pub(crate) fn sync_zone(
    client: &Client,
    zone: &Zone,
    machines: &[(Meta, Vec<IpAddr>)],
) -> ApiResult<usize> {
    // Desired (owner, type) -> records.
    let mut want: BTreeMap<(String, RecordType), Vec<String>> = BTreeMap::new();
    for (meta, addrs) in machines {
        if zone.meta.project.is_some() && meta.project != zone.meta.project {
            continue;
        }
        let owner = match normalize_name(&format!("{}.{}", meta.name, zone.spec.name)) {
            Ok(o) => o,
            Err(e) => {
                tracing::debug!(machine = %meta.name, error = %e, "beacon auto: name skipped");
                continue;
            }
        };
        let v4: Vec<String> = addrs
            .iter()
            .filter(|a| a.is_ipv4())
            .map(ToString::to_string)
            .collect();
        let v6: Vec<String> = addrs
            .iter()
            .filter(|a| a.is_ipv6())
            .map(ToString::to_string)
            .collect();
        if !v4.is_empty() {
            want.insert((owner.clone(), RecordType::A), v4);
        }
        if !v6.is_empty() {
            want.insert((owner, RecordType::Aaaa), v6);
        }
    }

    let mut changed = 0;
    let existing = client.recordsets_in_zone(&zone.meta.id)?;
    // Remove managed sets no longer wanted.
    for rs in &existing {
        let managed = rs.meta.labels.get(MANAGED_LABEL).map(String::as_str) == Some(OWNER);
        if managed && !want.contains_key(&(rs.spec.name.clone(), rs.spec.kind)) {
            client.delete_recordset(&rs.meta.id)?;
            changed += 1;
        }
    }
    // Upsert the rest. A user-owned set at the same (owner, type) is left
    // alone: hand-written data wins over automation.
    for ((owner, kind_), records) in want {
        if let Some(cur) = existing
            .iter()
            .find(|r| r.spec.name == owner && r.spec.kind == kind_)
        {
            if cur.meta.labels.get(MANAGED_LABEL).map(String::as_str) != Some(OWNER) {
                continue;
            }
            if cur.spec.records == records {
                continue;
            }
        }
        match client.upsert_record(&zone.meta.id, &owner, kind_, None, records, OWNER) {
            Ok(_) => changed += 1,
            Err(e) => tracing::warn!(owner, error = %e, "beacon auto: upsert failed"),
        }
    }
    Ok(changed)
}
