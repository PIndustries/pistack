// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use serde_json::json;
use tokio::time::Instant;

use crate::proto::{CommandId, CommandKind};
use crate::queue::test_support::*;

#[tokio::test]
async fn long_poll_receives_command_enqueued_later() {
    let q = CommandQueue::new(cfg(60));
    let q2 = q.clone();
    let poller = tokio::spawn(async move { q2.next("n1", Duration::from_secs(5), 16).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let t = q
        .enqueue("n1", CommandKind::StopMachine, json!(null))
        .unwrap();
    let cmds = poller.await.unwrap();
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].id, t.id());
}

#[tokio::test]
async fn long_poll_times_out_empty() {
    let q = CommandQueue::new(cfg(60));
    let start = Instant::now();
    let cmds = q.next("idle", Duration::from_millis(100), 16).await;
    assert!(cmds.is_empty());
    assert!(start.elapsed() >= Duration::from_millis(100));
}

#[tokio::test]
async fn batches_respect_max_and_order() {
    let q = CommandQueue::new(cfg(60));
    let ids: Vec<CommandId> = (0..3)
        .map(|_| {
            q.enqueue("n1", CommandKind::PlugPort, json!(null))
                .unwrap()
                .id()
        })
        .collect();
    let first = q.next("n1", Duration::ZERO, 2).await;
    assert_eq!(first.iter().map(|c| c.id).collect::<Vec<_>>(), ids[..2]);
    let rest = q.next("n1", Duration::ZERO, 2).await;
    assert_eq!(rest.iter().map(|c| c.id).collect::<Vec<_>>(), ids[2..]);
    assert!(q.next("n1", Duration::ZERO, 2).await.is_empty());
}

#[tokio::test]
async fn queues_are_per_node() {
    let q = CommandQueue::new(cfg(60));
    let a = q
        .enqueue("a", CommandKind::StartMachine, json!(null))
        .unwrap();
    let _b = q
        .enqueue("b", CommandKind::StartMachine, json!(null))
        .unwrap();
    let got = q.next("a", Duration::ZERO, 16).await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].id, a.id());
    assert_eq!(q.node("b").unwrap().queued, 1);
    assert_eq!(
        q.nodes()
            .iter()
            .map(|n| n.name.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
}
