// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use pistack_store::Store;
use serde_json::json;

use crate::error::RpcError;
use crate::queue::test_support::*;

#[tokio::test]
async fn a_result_for_a_command_in_flight_at_failover_reaches_the_new_leader() {
    let store = Store::in_memory().unwrap();
    let old = queue_on(&store, 60);
    let lost = old
        .enqueue_keyed("n1", "volume/v1", CommandKind::AttachVolume, json!(null))
        .unwrap();
    let id = lost.id();
    // The agent has it when the leader goes.
    assert_eq!(old.next("n1", Duration::ZERO, 16).await.len(), 1);
    drop((old, lost));

    let new = queue_on(&store, 60);
    // Before the module has issued it again the new leader cannot place
    // the result. The agent keeps it and answers the redelivery.
    let early = new.ack_all("n1", vec![CommandResult::ok(id)]);
    assert_eq!(early.accepted, 0);
    assert_eq!(early.unknown, vec![id]);
    assert_eq!(records(&store).len(), 1);

    let t = new
        .enqueue_keyed("n1", "volume/v1", CommandKind::AttachVolume, json!(null))
        .unwrap();
    assert_eq!(t.id(), id);
    let again = new.next("n1", Duration::ZERO, 16).await;
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].id, id);
    let ack = new.ack_all(
        "n1",
        vec![CommandResult::ok(id).with_payload(json!({"device": "vdb"}))],
    );
    assert_eq!(ack.accepted, 1);
    assert_eq!(t.wait().await.unwrap().payload["device"], "vdb");
    assert!(records(&store).is_empty());
}

#[tokio::test]
async fn a_result_that_arrives_before_the_redelivery_is_taken_once() {
    let store = Store::in_memory().unwrap();
    let old = queue_on(&store, 60);
    let id = old
        .enqueue_keyed("n1", "machine/m1", CommandKind::StopMachine, json!(null))
        .unwrap()
        .id();
    assert_eq!(old.next("n1", Duration::ZERO, 16).await.len(), 1);
    drop(old);

    let new = queue_on(&store, 60);
    let t = new
        .enqueue_keyed("n1", "machine/m1", CommandKind::StopMachine, json!(null))
        .unwrap();
    // The agent reports what it ran for the old leader.
    new.ack("n1", CommandResult::ok(id)).unwrap();
    assert!(t.wait().await.unwrap().ok);
    // The copy on the FIFO is no longer pending and is not handed out.
    assert!(new.next("n1", Duration::ZERO, 16).await.is_empty());
    assert!(matches!(
        new.ack("n1", CommandResult::ok(id)),
        Err(RpcError::UnknownCommand(_))
    ));
}
