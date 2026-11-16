// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use chrono::Utc;
use pistack_types::Resources;
use serde_json::json;

use crate::error::RpcError;
use crate::proto::CommandResult;
use crate::queue::test_support::*;

#[tokio::test(start_paused = true)]
async fn delivered_but_unanswered_times_out_via_reap() {
    let q = CommandQueue::new(cfg(10));
    let t = q
        .enqueue("n1", CommandKind::StartMachine, json!(null))
        .unwrap();
    assert_eq!(q.next("n1", Duration::ZERO, 16).await.len(), 1);
    tokio::time::advance(Duration::from_secs(11)).await;
    // Any queue operation reaps; here an explicit call.
    assert_eq!(q.reap(), 1);
    assert_eq!(q.pending_count(), 0);
    let e = t.wait().await.unwrap_err();
    assert!(matches!(e, RpcError::Timeout { .. }));
    // A late result is rejected.
    assert!(matches!(
        q.ack("n1", CommandResult::ok(e_id(&e))),
        Err(RpcError::UnknownCommand(_))
    ));
}

#[tokio::test(start_paused = true)]
async fn reap_is_triggered_by_other_operations() {
    let q = CommandQueue::new(cfg(10));
    let t = q
        .enqueue("n1", CommandKind::StartMachine, json!(null))
        .unwrap();
    tokio::time::advance(Duration::from_secs(10)).await;
    // A heartbeat from any node reaps expired commands.
    let ack = q.heartbeat(&Heartbeat {
        node_name: "n2".into(),
        resources_free: Resources::default(),
        machines: vec![],
        at: Utc::now(),
        ovn: None,
        usage: None,
    });
    assert!(!ack.known);
    assert_eq!(q.pending_count(), 0);
    assert!(matches!(t.wait().await, Err(RpcError::Timeout { .. })));
}
