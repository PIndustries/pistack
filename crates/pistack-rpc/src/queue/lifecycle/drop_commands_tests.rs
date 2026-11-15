// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use pistack_store::Store;
use serde_json::json;

use crate::error::RpcError;
use crate::proto::CommandKind;
use crate::queue::test_support::*;

#[tokio::test]
async fn dropping_a_nodes_commands_fails_them_and_removes_its_records() {
    let store = Store::in_memory().unwrap();
    let old = queue_on(&store, 60);
    // One left by an earlier leader, one pending here, one for another node.
    let _left = old
        .enqueue_keyed("gone", "machine/m1", CommandKind::StartMachine, json!(null))
        .unwrap();
    let q = queue_on(&store, 60);
    let t = q
        .enqueue_keyed("gone", "volume/v1", CommandKind::AttachVolume, json!(null))
        .unwrap();
    let _other = q
        .enqueue_keyed(
            "stays",
            "machine/m2",
            CommandKind::StartMachine,
            json!(null),
        )
        .unwrap();
    assert_eq!(records(&store).len(), 3);

    assert_eq!(q.drop_commands("gone"), 1);
    assert!(matches!(
        t.wait().await,
        Err(RpcError::NodeRemoved { ref node, .. }) if node == "gone"
    ));
    assert_eq!(records(&store), vec!["rpc/commands/stays/machine/m2"]);
    // The host is still known, and gets nothing of what was dropped.
    assert!(q.node("gone").is_some());
    assert!(q.next("gone", Duration::ZERO, 16).await.is_empty());
    // A command issued afterwards is a new one.
    let again = q
        .enqueue_keyed("gone", "machine/m1", CommandKind::StartMachine, json!(null))
        .unwrap();
    assert_ne!(again.id(), _left.id());
}
