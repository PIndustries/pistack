// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use serde_json::json;

use crate::error::RpcError;
use crate::proto::CommandKind;
use crate::queue::test_support::*;

#[tokio::test(start_paused = true)]
async fn ticket_times_out_when_undelivered() {
    let q = CommandQueue::new(cfg(10));
    let t = q
        .enqueue("offline", CommandKind::DeleteMachine, json!(null))
        .unwrap();
    let id = t.id();
    let e = t.wait().await.unwrap_err();
    assert!(
        matches!(e, RpcError::Timeout { id: tid, kind: CommandKind::DeleteMachine, ref node } if tid == id && node == "offline"),
        "{e}"
    );
    assert_eq!(q.pending_count(), 0);
    // The stale command is still in the channel but is skipped on poll.
    assert!(q.next("offline", Duration::ZERO, 16).await.is_empty());
    assert_eq!(q.node("offline").unwrap().queued, 0);
}
