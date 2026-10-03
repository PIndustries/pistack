// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Automatic machine records.
//!
//! A zone labelled `pistack.io/auto=true` gets an `A` (and, when the machine
//! has IPv6, an `AAAA`) set `<machine-name>.<zone>` for every machine in the
//! zone's project. The address is the machine's floating IP when one is
//! associated with any of its ports, otherwise its fixed addresses as
//! reported in `status.addresses`. Sets beacon writes carry
//! `pistack.io/managed-by=beacon-auto`; sets with that label whose machine
//! has gone are removed. A pass is idempotent, so it runs on every `mach`,
//! `fip` and `port` event and on a timer with no harm.
//!
//! `forge` and `fabric` own machines, ports and floating IPs; beacon reads
//! them through the store's JSON wire shape with narrow views below, so it
//! does not depend on those crates and cannot write to their resources.

use crate::client::Client;
use crate::types::{normalize_name, RecordType, Zone, ZoneKind, AUTO_LABEL, MANAGED_LABEL};
use pistack_types::module::CancellationToken;
use pistack_types::{kind, ApiResult, Event, Id, Meta};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::IpAddr;
use std::time::Duration;
use tokio::sync::broadcast;

/// Value of `pistack.io/managed-by` on sets this task owns.
pub const OWNER: &str = "beacon-auto";
/// Full pass interval, catching anything an event missed.
const RESYNC: Duration = Duration::from_secs(60);
/// Events that may change a machine's name or addresses.
const TRIGGERS: &[&str] = &[kind::MACHINE, kind::FLOATING_IP, kind::PORT, kind::ZONE];

#[derive(Deserialize)]
struct MachineView {
    #[serde(flatten)]
    meta: Meta,
    #[serde(default)]
    status: MachineStatusView,
}

#[derive(Deserialize, Default)]
struct MachineStatusView {
    #[serde(default)]
    addresses: Vec<String>,
}

#[derive(Deserialize)]
struct PortView {
    #[serde(flatten)]
    meta: Meta,
    spec: PortSpecView,
}

#[derive(Deserialize)]
struct PortSpecView {
    #[serde(default)]
    device: Option<Id>,
}

#[derive(Deserialize)]
struct FloatingIpView {
    spec: FloatingIpSpecView,
}

#[derive(Deserialize)]
struct FloatingIpSpecView {
    #[serde(default)]
    address: Option<IpAddr>,
    #[serde(default)]
    port: Option<Id>,
}

/// Every stored resource of `kind_`, decoded as `T`. Rows that do not
/// decode are skipped with a warning rather than failing the pass.
fn scan<T: serde::de::DeserializeOwned>(client: &Client, kind_: &str) -> ApiResult<Vec<T>> {
    let mut out = Vec::new();
    for (key, bytes) in client.store().raw_scan("resources", &format!("{kind_}/"))? {
        match serde_json::from_slice::<T>(&bytes) {
            Ok(v) => out.push(v),
            Err(e) => tracing::warn!(key, error = %e, "beacon auto: unreadable resource"),
        }
    }
    Ok(out)
}

/// Machine id -> addresses beacon should publish for it.
fn machine_addresses(client: &Client) -> ApiResult<Vec<(Meta, Vec<IpAddr>)>> {
    let machines: Vec<MachineView> = scan(client, kind::MACHINE)?;
    let ports: Vec<PortView> = scan(client, kind::PORT)?;
    let fips: Vec<FloatingIpView> = scan(client, kind::FLOATING_IP)?;

    let port_device: HashMap<Id, Id> = ports
        .into_iter()
        .filter_map(|p| p.spec.device.map(|d| (p.meta.id, d)))
        .collect();
    let mut floating: HashMap<Id, BTreeSet<IpAddr>> = HashMap::new();
    for f in fips {
        if let (Some(port), Some(addr)) = (f.spec.port, f.spec.address) {
            if let Some(dev) = port_device.get(&port) {
                floating.entry(dev.clone()).or_default().insert(addr);
            }
        }
    }

    let mut out = Vec::with_capacity(machines.len());
    for m in machines {
        let addrs: Vec<IpAddr> = match floating.get(&m.meta.id) {
            Some(f) if !f.is_empty() => f.iter().copied().collect(),
            _ => {
                let mut v: Vec<IpAddr> = m
                    .status
                    .addresses
                    .iter()
                    .filter_map(|a| a.split('/').next()?.trim().parse().ok())
                    .collect();
                v.sort();
                v.dedup();
                v
            }
        };
        out.push((m.meta, addrs));
    }
    Ok(out)
}

/// Zones that opted in.
fn auto_zones(client: &Client) -> ApiResult<Vec<Zone>> {
    Ok(client
        .zones()
        .list(None)?
        .into_iter()
        .filter(|z| {
            z.spec.kind == ZoneKind::Primary
                && z.meta.labels.get(AUTO_LABEL).map(String::as_str) == Some("true")
        })
        .collect())
}

/// One full pass over every auto zone. Returns the number of sets written
/// or removed.
pub fn sync_all(client: &Client) -> ApiResult<usize> {
    let zones = auto_zones(client)?;
    if zones.is_empty() {
        return Ok(0);
    }
    let machines = machine_addresses(client)?;
    let mut changed = 0;
    for zone in zones {
        changed += sync_zone(client, &zone, &machines)?;
    }
    Ok(changed)
}

fn sync_zone(client: &Client, zone: &Zone, machines: &[(Meta, Vec<IpAddr>)]) -> ApiResult<usize> {
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

/// Run `sync_all` now, then on every triggering event and every `RESYNC`,
/// until `shutdown` fires.
pub fn start(client: Client, mut rx: broadcast::Receiver<Event>, shutdown: CancellationToken) {
    tokio::spawn(async move {
        run_pass(&client);
        let mut tick = tokio::time::interval(RESYNC);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::debug!("beacon auto-records stopping");
                    break;
                }
                ev = rx.recv() => match ev {
                    Ok(ev) if TRIGGERS.contains(&ev.kind.as_str()) => run_pass(&client),
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => run_pass(&client),
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = tick.tick() => run_pass(&client),
            }
        }
    });
}

fn run_pass(client: &Client) {
    match sync_all(client) {
        Ok(0) => {}
        Ok(n) => tracing::info!(changed = n, "beacon auto-records synced"),
        Err(e) => tracing::warn!(error = %e, "beacon auto-records pass failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ZoneSpec;
    use pistack_store::Store;
    use serde_json::json;

    /// Write a raw resource the way forge/fabric would.
    fn put_raw(store: &Store, kind_: &str, id: &Id, value: serde_json::Value) {
        let col = store.collection::<RawRes>();
        let mut v = value;
        v["id"] = json!(id.to_string());
        let r: RawRes = serde_json::from_value(v).unwrap();
        if col.try_get(id).unwrap().is_some() {
            col.update(r, None).unwrap();
        } else {
            col.create(r).unwrap();
        }
        let _ = kind_;
    }

    #[derive(Clone, serde::Serialize, Deserialize)]
    struct RawRes {
        #[serde(flatten)]
        meta: Meta,
        #[serde(default)]
        spec: serde_json::Value,
        #[serde(default)]
        status: serde_json::Value,
    }
    impl pistack_types::Kind for RawRes {
        const KIND: &'static str = "mach";
        fn meta(&self) -> &Meta {
            &self.meta
        }
        fn meta_mut(&mut self) -> &mut Meta {
            &mut self.meta
        }
    }

    #[test]
    fn machines_get_a_records_and_lose_them() {
        let store = Store::in_memory().unwrap();
        let client = Client::new(store.clone());
        let project = Id::new(kind::PROJECT);
        let mut meta = Meta::new(kind::ZONE, "example", Some(project.clone()));
        meta.labels.insert(AUTO_LABEL.into(), "true".into());
        let zone = client
            .create_zone(
                meta,
                ZoneSpec {
                    name: "example.cloud.".into(),
                    ..ZoneSpec::default()
                },
            )
            .unwrap();

        let m = Meta::new(kind::MACHINE, "web", Some(project.clone()));
        let mut value = serde_json::to_value(&m).unwrap();
        value["spec"] = json!({});
        value["status"] = json!({ "addresses": ["10.0.0.4/24", "fd00::4"] });
        put_raw(&store, kind::MACHINE, &m.id, value.clone());

        assert_eq!(sync_all(&client).unwrap(), 2);
        let sets = client.recordsets_in_zone(&zone.meta.id).unwrap();
        assert_eq!(sets.len(), 2);
        let a = sets.iter().find(|s| s.spec.kind == RecordType::A).unwrap();
        assert_eq!(a.spec.name, "web.example.cloud.");
        assert_eq!(a.spec.records, vec!["10.0.0.4"]);
        assert_eq!(a.meta.labels.get(MANAGED_LABEL).unwrap(), OWNER);

        // idempotent
        assert_eq!(sync_all(&client).unwrap(), 0);

        // address change
        value["status"] = json!({ "addresses": ["10.0.0.5"] });
        put_raw(&store, kind::MACHINE, &m.id, value);
        assert_eq!(sync_all(&client).unwrap(), 2); // A updated, AAAA removed
        let sets = client.recordsets_in_zone(&zone.meta.id).unwrap();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].spec.records, vec!["10.0.0.5"]);

        // machine gone
        store.collection::<RawRes>().delete(&m.id).unwrap();
        assert_eq!(sync_all(&client).unwrap(), 1);
        assert!(client.recordsets_in_zone(&zone.meta.id).unwrap().is_empty());

        // zone without the label is untouched
        let plain = client
            .create_zone(
                Meta::new(kind::ZONE, "plain", Some(project)),
                ZoneSpec {
                    name: "plain.cloud.".into(),
                    ..ZoneSpec::default()
                },
            )
            .unwrap();
        assert_eq!(sync_all(&client).unwrap(), 0);
        assert!(client
            .recordsets_in_zone(&plain.meta.id)
            .unwrap()
            .is_empty());
    }
}
