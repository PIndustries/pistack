// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use serde_json::json;

use crate::proto::CommandKind;
use crate::queue::test_support::*;

#[tokio::test]
async fn enqueue_then_poll_then_ack() {
    let q = CommandQueue::new(cfg(60));
    let t = q
        .enqueue("n1", CommandKind::StartMachine, json!({"machine": "web"}))
        .unwrap();
    let id = t.id();
    assert_eq!(q.pending_count(), 1);
    assert_eq!(q.node("n1").unwrap().queued, 1);

    let cmds = q.next("n1", Duration::from_millis(10), 16).await;
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].id, id);
    assert_eq!(cmds[0].kind, CommandKind::StartMachine);
    assert_eq!(cmds[0].payload["machine"], "web");
    let info = q.node("n1").unwrap();
    assert_eq!(info.queued, 0);
    assert_eq!(info.in_flight, 1);
    assert!(q.pending()[0].delivered);

    q.ack("n1", CommandResult::ok(id).with_payload(json!({"pid": 42})))
        .unwrap();
    let r = t.wait().await.unwrap();
    assert!(r.ok);
    assert_eq!(r.payload["pid"], 42);
    assert_eq!(q.pending_count(), 0);
    assert_eq!(q.node("n1").unwrap().in_flight, 0);
}
