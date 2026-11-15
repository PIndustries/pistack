// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde_json::json;

use crate::error::RpcError;
use crate::proto::CommandKind;
use crate::queue::test_support::*;

#[tokio::test]
async fn remove_node_fails_pending() {
    let q = CommandQueue::new(cfg(60));
    let t = q
        .enqueue("n1", CommandKind::StartMachine, json!(null))
        .unwrap();
    q.remove_node("n1");
    assert!(q.node("n1").is_none());
    let e = t.wait().await.unwrap_err();
    assert!(
        matches!(e, RpcError::NodeRemoved { ref node, .. } if node == "n1"),
        "{e}"
    );
}
