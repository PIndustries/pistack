// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_store::Store;
use pistack_types::{kind, Meta, Phase};

use super::*;

fn client() -> Client {
    Client::new(Store::in_memory().unwrap())
}

fn zone(c: &Client, name: &str) -> Zone {
    c.create_zone(
        Meta::new(kind::ZONE, name.trim_end_matches('.'), None),
        ZoneSpec {
            name: name.into(),
            ..ZoneSpec::default()
        },
    )
    .unwrap()
}

#[test]
fn cname_exclusivity_and_uniqueness() {
    let c = client();
    let z = zone(&c, "example.cloud.");
    let a = |name: &str, k: RecordType, recs: &[&str]| RecordSetSpec {
        zone: z.meta.id.clone(),
        name: name.into(),
        kind: k,
        ttl: None,
        records: recs.iter().map(|s| s.to_string()).collect(),
    };
    c.create_recordset(
        Meta::new(kind::RECORD, "web-a", None),
        a("web", RecordType::A, &["10.0.0.4"]),
    )
    .unwrap();
    // same owner+type twice
    let err = c
        .create_recordset(
            Meta::new(kind::RECORD, "web-a2", None),
            a("web", RecordType::A, &["10.0.0.5"]),
        )
        .unwrap_err();
    assert_eq!(err.code, pistack_types::error::ErrorCode::Conflict);
    // CNAME next to an A
    let err = c
        .create_recordset(
            Meta::new(kind::RECORD, "web-c", None),
            a("web", RecordType::Cname, &["other.example.cloud."]),
        )
        .unwrap_err();
    assert_eq!(err.code, pistack_types::error::ErrorCode::Conflict);
    // A next to a CNAME
    c.create_recordset(
        Meta::new(kind::RECORD, "www-c", None),
        a("www", RecordType::Cname, &["web.example.cloud."]),
    )
    .unwrap();
    assert!(c
        .create_recordset(
            Meta::new(kind::RECORD, "www-txt", None),
            a("www", RecordType::Txt, &["x"])
        )
        .is_err());
    // Different type at same owner is fine.
    c.create_recordset(
        Meta::new(kind::RECORD, "web-txt", None),
        a("web", RecordType::Txt, &["x"]),
    )
    .unwrap();
}

#[test]
fn upsert_is_idempotent_and_serial_moves_only_on_change() {
    let c = client();
    let z = zone(&c, "example.cloud.");
    let first = c
        .upsert_record(
            &z.meta.id,
            "web",
            RecordType::A,
            None,
            vec!["10.0.0.4".into()],
            "test",
        )
        .unwrap();
    assert_eq!(first.meta.labels.get(MANAGED_LABEL).unwrap(), "test");
    let again = c
        .upsert_record(
            &z.meta.id,
            "web",
            RecordType::A,
            None,
            vec!["10.0.0.4".into()],
            "test",
        )
        .unwrap();
    assert_eq!(again.meta.revision, first.meta.revision);

    let z1 = c.reconcile_zone(&z.meta.id).unwrap().unwrap();
    assert_eq!(z1.status.serial, 1);
    assert_eq!(z1.status.records, 1);
    let z2 = c.reconcile_zone(&z.meta.id).unwrap().unwrap();
    assert_eq!(z2.status.serial, 1);

    let changed = c
        .upsert_record(
            &z.meta.id,
            "web",
            RecordType::A,
            None,
            vec!["10.0.0.5".into()],
            "test",
        )
        .unwrap();
    assert_eq!(changed.meta.id, first.meta.id);
    assert_eq!(
        c.reconcile_zone(&z.meta.id).unwrap().unwrap().status.serial,
        2
    );

    assert!(c
        .remove_record(&z.meta.id, "web", RecordType::A)
        .unwrap()
        .is_some());
    assert!(c
        .remove_record(&z.meta.id, "web", RecordType::A)
        .unwrap()
        .is_none());
    assert_eq!(
        c.reconcile_zone(&z.meta.id).unwrap().unwrap().status.serial,
        3
    );
}

#[test]
fn secondary_stays_pending_and_rejects_records() {
    let c = client();
    let z = c
        .create_zone(
            Meta::new(kind::ZONE, "sec", None),
            ZoneSpec {
                name: "sec.cloud.".into(),
                kind: ZoneKind::Secondary {
                    primaries: vec!["10.0.0.1:53".into()],
                },
                ..ZoneSpec::default()
            },
        )
        .unwrap();
    let out = c.reconcile_zone(&z.meta.id).unwrap().unwrap();
    assert_eq!(out.status.phase, Phase::Pending);
    assert_eq!(out.status.message, SECONDARY_PENDING_MSG);
    assert_eq!(out.status.serial, 0);
    assert!(c
        .create_recordset(
            Meta::new(kind::RECORD, "a", None),
            RecordSetSpec {
                zone: z.meta.id.clone(),
                name: "web".into(),
                kind: RecordType::A,
                ttl: None,
                records: vec!["10.0.0.4".into()],
            },
        )
        .is_err());
    // Idempotent reconcile — does not clobber.
    let again = c.reconcile_zone(&z.meta.id).unwrap().unwrap();
    assert_eq!(again.status.phase, Phase::Pending);
    assert_eq!(again.status.message, SECONDARY_PENDING_MSG);
    assert_eq!(again.meta.revision, out.meta.revision);
    // Transfer success marks Ready.
    let ready = c
        .secondary_ready(&z.meta.id, 7, 3, "abc".into(), "10.0.0.1:53")
        .unwrap();
    assert_eq!(ready.status.phase, Phase::Ready);
    assert_eq!(ready.status.serial, 7);
    assert!(ready.status.message.contains("10.0.0.1:53"));
    // Reconcile must not wipe Ready.
    let kept = c.reconcile_zone(&z.meta.id).unwrap().unwrap();
    assert_eq!(kept.status.phase, Phase::Ready);
    assert_eq!(kept.status.serial, 7);
}

#[test]
fn zone_lookup_prefers_longest_apex() {
    let c = client();
    let parent = zone(&c, "cloud.");
    let child = zone(&c, "example.cloud.");
    assert_eq!(
        c.zone_for_name("web.example.cloud.")
            .unwrap()
            .unwrap()
            .meta
            .id,
        child.meta.id
    );
    assert_eq!(
        c.zone_for_name("x.cloud.").unwrap().unwrap().meta.id,
        parent.meta.id
    );
    assert!(c.zone_for_name("other.").unwrap().is_none());
    // duplicate apex refused
    assert!(c
        .create_zone(
            Meta::new(kind::ZONE, "dup", None),
            ZoneSpec {
                name: "Example.Cloud".into(),
                ..ZoneSpec::default()
            }
        )
        .is_err());
    // delete cascades
    c.upsert_record(
        &child.meta.id,
        "web",
        RecordType::A,
        None,
        vec!["10.0.0.4".into()],
        "t",
    )
    .unwrap();
    c.delete_zone(&child.meta.id).unwrap();
    assert!(c.recordsets().list(None).unwrap().is_empty());
}
