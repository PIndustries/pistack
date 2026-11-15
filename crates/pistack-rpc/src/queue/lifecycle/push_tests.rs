// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use serde_json::json;

use crate::error::RpcError;
use crate::proto::CommandKind;
use crate::queue::test_support::*;

#[tokio::test]
async fn queue_full() {
    let q = CommandQueue::new(cfg(60));
    let mut tickets = Vec::new();
    for _ in 0..4 {
        tickets.push(
            q.enqueue("n1", CommandKind::StartMachine, json!(null))
                .unwrap(),
        );
    }
    let e = q
        .enqueue("n1", CommandKind::StartMachine, json!(null))
        .unwrap_err();
    assert!(matches!(e, RpcError::QueueFull { ref node } if node == "n1"));
    assert_eq!(e.to_string(), "command queue for node n1 is full");
    // Draining makes room; pending stays until acked.
    assert_eq!(q.next("n1", Duration::ZERO, 16).await.len(), 4);
    assert!(q
        .enqueue("n1", CommandKind::StartMachine, json!(null))
        .is_ok());
    assert_eq!(q.pending_count(), 5);
}
