// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::{RecordType, AUTO_LABEL, MANAGED_LABEL};
use pistack_types::{kind, Id, Meta};
use serde::Deserialize;

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
