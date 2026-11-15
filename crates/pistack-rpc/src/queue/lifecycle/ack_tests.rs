// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use serde_json::json;
use uuid::Uuid;

use crate::error::RpcError;
use crate::proto::CommandResult;
use crate::queue::test_support::*;

#[tokio::test]
async fn ack_unknown_or_wrong_node_is_rejected() {
    let q = CommandQueue::new(cfg(60));
    let t = q
        .enqueue("n1", CommandKind::AttachVolume, json!(null))
        .unwrap();
    let _ = q.next("n1", Duration::ZERO, 16).await;
    let e = q.ack("n2", CommandResult::ok(t.id())).unwrap_err();
    assert!(matches!(e, RpcError::UnknownCommand(id) if id == t.id()));
    let e = q.ack("n1", CommandResult::ok(Uuid::now_v7())).unwrap_err();
    assert!(matches!(e, RpcError::UnknownCommand(_)));
    // The right node still works, once.
    q.ack("n1", CommandResult::failed(t.id(), "no such volume"))
        .unwrap();
    let e = q.ack("n1", CommandResult::ok(t.id())).unwrap_err();
    assert!(matches!(e, RpcError::UnknownCommand(_)));
    let r = t.wait().await.unwrap();
    assert!(!r.ok);
    assert_eq!(r.message, "no such volume");
}
