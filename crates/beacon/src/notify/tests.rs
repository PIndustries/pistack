// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::ZoneKind;
use hickory_proto::op::{Message, MessageType, OpCode};
use hickory_proto::rr::{RData, RecordType};
use std::net::SocketAddr;

use super::notified_key::NOTIFIED_KEY;
use super::notify_bytes::notify_bytes;
use super::notify_moved::notify_moved;
use super::*;
use crate::types::ZoneSpec;
use pistack_store::Store;
use pistack_types::{kind, Meta};

fn client() -> Client {
    Client::new(Store::in_memory().unwrap())
}

fn secondary(c: &Client, name: &str, apex: &str, primaries: &[&str]) -> crate::types::Zone {
    c.create_zone(
        Meta::new(kind::ZONE, name, None),
        ZoneSpec {
            name: apex.into(),
            kind: ZoneKind::Secondary {
                primaries: primaries.iter().map(|s| s.to_string()).collect(),
            },
            ..ZoneSpec::default()
        },
    )
    .unwrap()
}

#[test]
fn addresses_accept_both_forms() {
    assert_eq!(
        parse_addr("10.0.0.1", 53).unwrap(),
        "10.0.0.1:53".parse().unwrap()
    );
    assert_eq!(
        parse_addr("127.0.0.1:15353", 53).unwrap(),
        "127.0.0.1:15353".parse().unwrap()
    );
    assert_eq!(
        parse_addr("[::1]:5353", 53).unwrap(),
        "[::1]:5353".parse().unwrap()
    );
    assert!(parse_addr("ns1.example.cloud", 53).is_err());
}

fn primary(c: &Client, name: &str, apex: &str, also: &[&str]) -> crate::types::Zone {
    c.create_zone(
        Meta::new(kind::ZONE, name, None),
        ZoneSpec {
            name: apex.into(),
            also_notify: also.iter().map(|s| s.to_string()).collect(),
            ..ZoneSpec::default()
        },
    )
    .unwrap()
}

#[test]
fn peers_come_from_secondary_primaries() {
    let c = client();
    secondary(&c, "sec-a", "a.cloud.", &["127.0.0.1:15353"]);
    secondary(&c, "sec-b", "b.cloud.", &["10.0.0.2"]);
    assert_eq!(
        peers_for(&c, &primary(&c, "prim-a", "a.cloud.", &[])).unwrap(),
        vec!["127.0.0.1:15353".parse::<SocketAddr>().unwrap()]
    );
    // A bare address means the standard port.
    assert_eq!(
        peers_for(&c, &primary(&c, "prim-b", "b.cloud.", &[])).unwrap(),
        vec!["10.0.0.2:53".parse::<SocketAddr>().unwrap()]
    );
    assert!(peers_for(&c, &primary(&c, "prim-c", "other.cloud.", &[]))
        .unwrap()
        .is_empty());
}

/// A secondary that is an outside DNS server holds nothing in the store, so
/// the zone names it directly. Both lists are asked at once and an address
/// named twice is asked once.
#[test]
fn also_notify_addresses_are_added() {
    let c = client();
    secondary(&c, "sec", "d.cloud.", &["10.0.0.9", "10.0.0.10"]);
    let z = primary(&c, "prim", "d.cloud.", &["198.51.100.7", "10.0.0.10"]);
    assert_eq!(
        peers_for(&c, &z).unwrap(),
        vec![
            "198.51.100.7:53".parse::<SocketAddr>().unwrap(),
            "10.0.0.10:53".parse::<SocketAddr>().unwrap(),
            "10.0.0.9:53".parse::<SocketAddr>().unwrap(),
        ]
    );
}

#[test]
fn notify_message_is_a_notify_for_the_apex() {
    let bytes = notify_bytes(0xabcd, "example.cloud.", 9).unwrap();
    let msg = Message::from_vec(&bytes).unwrap();
    assert_eq!(msg.metadata.id, 0xabcd);
    assert_eq!(msg.metadata.op_code, OpCode::Notify);
    assert_eq!(msg.metadata.message_type, MessageType::Query);
    assert!(msg.metadata.authoritative);
    assert_eq!(msg.queries.len(), 1);
    assert_eq!(msg.queries[0].name().to_ascii(), "example.cloud.");
    assert_eq!(msg.queries[0].query_type(), RecordType::SOA);
    assert_eq!(msg.answers.len(), 1);
    match &msg.answers[0].data {
        RData::SOA(soa) => assert_eq!(soa.serial, 9),
        other => panic!("the hint should be SOA, got {other:?}"),
    }
    assert_eq!(asked_apex(&msg).as_deref(), Some("example.cloud."));
}

#[test]
fn only_a_listed_primary_may_notify() {
    let c = client();
    secondary(&c, "sec", "x.cloud.", &["127.0.0.1"]);
    let msg = Message::from_vec(&notify_bytes(1, "x.cloud.", 3).unwrap()).unwrap();
    // A NOTIFY arrives from an ephemeral source port, so the address a
    // secondary lists is matched on host alone: two primaries packed onto
    // one host are one principal as far as a NOTIFY can tell.
    assert!(notify_allowed(&c, &msg, "127.0.0.1:53".parse().unwrap()).unwrap());
    assert!(notify_allowed(&c, &msg, "127.0.0.1:15353".parse().unwrap()).unwrap());
    // Another host, a zone we do not hold, and a plain query all fail.
    assert!(!notify_allowed(&c, &msg, "10.9.9.9:53".parse().unwrap()).unwrap());
    let other = Message::from_vec(&notify_bytes(1, "y.cloud.", 3).unwrap()).unwrap();
    assert!(!notify_allowed(&c, &other, "127.0.0.1:53".parse().unwrap()).unwrap());
    let q = Message::query();
    assert!(!notify_allowed(&c, &q, "127.0.0.1:53".parse().unwrap()).unwrap());
}

#[tokio::test]
async fn an_unanswered_notify_is_not_recorded() {
    let c = client();
    let z = c
        .create_zone(
            Meta::new(kind::ZONE, "prim", None),
            ZoneSpec {
                name: "gone.cloud.".into(),
                ..ZoneSpec::default()
            },
        )
        .unwrap();
    c.reconcile_zone(&z.meta.id).unwrap();
    // One listed peer, nothing listening at it. The retries burn the
    // backoff, which is the point: it must end, and it must end unmarked.
    secondary(&c, "gone-secondary", "gone.cloud.", &["127.0.0.1:1"]);
    notify_moved(&c).await;
    let after = c.zones().get(&z.meta.id).unwrap();
    assert!(
        !after.meta.annotations.contains_key(NOTIFIED_KEY),
        "an unanswered notify must not be recorded: {:?}",
        after.meta.annotations
    );
}

#[tokio::test]
async fn a_zone_nobody_listed_is_marked_immediately() {
    let c = client();
    let z = c
        .create_zone(
            Meta::new(kind::ZONE, "solo", None),
            ZoneSpec {
                name: "solo.cloud.".into(),
                ..ZoneSpec::default()
            },
        )
        .unwrap();
    c.reconcile_zone(&z.meta.id).unwrap();
    notify_moved(&c).await;
    let after = c.zones().get(&z.meta.id).unwrap();
    assert_eq!(
        after.meta.annotations.get(NOTIFIED_KEY).map(|s| s.as_str()),
        Some("1")
    );
    // A second pass has nothing to do.
    notify_moved(&c).await;
    assert_eq!(
        c.zones().get(&z.meta.id).unwrap().meta.revision,
        after.meta.revision
    );
}
