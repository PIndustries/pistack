// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use pistack_store::Store;
use serde_json::json;

use crate::error::RpcError;
use crate::proto::CommandKind;
use crate::queue::test_support::*;

#[tokio::test]
async fn a_keyed_command_keeps_its_id_when_the_queue_restarts() {
    let store = Store::in_memory().unwrap();
    let first = queue_on(&store, 60);
    let lost = first
        .enqueue_keyed("n1", "machine/m1", CommandKind::StartMachine, json!(1))
        .unwrap();
    let id = lost.id();
    assert_eq!(records(&store), vec!["rpc/commands/n1/machine/m1"]);
    drop((first, lost));

    // The new queue holds no commands; the module issues its own again.
    let second = queue_on(&store, 60);
    assert!(second.next("n1", Duration::ZERO, 16).await.is_empty());
    let t = second
        .enqueue_keyed("n1", "machine/m1", CommandKind::StartMachine, json!(2))
        .unwrap();
    assert_eq!(t.id(), id);
    let cmds = second.next("n1", Duration::ZERO, 16).await;
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].id, id);
    assert!(cmds[0].remember);
    assert_eq!(cmds[0].payload, json!(2));
    second.ack("n1", CommandResult::ok(id)).unwrap();
    assert!(t.wait().await.unwrap().ok);
    assert!(records(&store).is_empty());
    assert!(second.next("n1", Duration::ZERO, 16).await.is_empty());
}

#[tokio::test]
async fn the_same_key_twice_is_one_command() {
    let store = Store::in_memory().unwrap();
    let q = queue_on(&store, 60);
    let a = q
        .enqueue_keyed("n1", "machine/m1", CommandKind::DeleteMachine, json!(null))
        .unwrap();
    let b = q
        .enqueue_keyed("n1", "machine/m1", CommandKind::DeleteMachine, json!(null))
        .unwrap();
    assert_eq!(a.id(), b.id());
    assert!(matches!(a.wait().await, Err(RpcError::Closed)));
    assert_eq!(q.pending_count(), 1);
    let cmds = q.next("n1", Duration::ZERO, 16).await;
    assert_eq!(cmds.len(), 1);
    q.ack("n1", CommandResult::ok(cmds[0].id)).unwrap();
    assert!(b.wait().await.unwrap().ok);

    // Answered: the next one for the key is a new command.
    let c = q
        .enqueue_keyed("n1", "machine/m1", CommandKind::DeleteMachine, json!(null))
        .unwrap();
    assert_ne!(c.id(), cmds[0].id);
}

#[tokio::test]
async fn another_kind_for_the_key_is_a_new_command() {
    let store = Store::in_memory().unwrap();
    let q = queue_on(&store, 60);
    let stop = q
        .enqueue_keyed("n1", "machine/m1", CommandKind::StopMachine, json!(null))
        .unwrap();
    let stop_id = stop.id();
    let start = q
        .enqueue_keyed("n1", "machine/m1", CommandKind::StartMachine, json!(null))
        .unwrap();
    assert_ne!(start.id(), stop_id);
    assert!(matches!(stop.wait().await, Err(RpcError::Closed)));
    // Only the start is handed out, and one record is kept for the key.
    let cmds = q.next("n1", Duration::ZERO, 16).await;
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].id, start.id());
    assert_eq!(records(&store).len(), 1);

    // The same across a restart: a stop left by the old leader is not
    // the id of the start the new one issues.
    let store = Store::in_memory().unwrap();
    let old = queue_on(&store, 60);
    let left = old
        .enqueue_keyed("n1", "machine/m1", CommandKind::StopMachine, json!(null))
        .unwrap()
        .id();
    let new = queue_on(&store, 60);
    let start = new
        .enqueue_keyed("n1", "machine/m1", CommandKind::StartMachine, json!(null))
        .unwrap();
    assert_ne!(start.id(), left);
    let stop = new
        .enqueue_keyed("n1", "machine/m1", CommandKind::StopMachine, json!(null))
        .unwrap();
    assert_ne!(stop.id(), left);
}

#[tokio::test(start_paused = true)]
async fn a_host_that_stays_down_does_not_grow_the_store() {
    let store = Store::in_memory().unwrap();
    // Capacity 4, timeout 10 s, and an agent that never polls.
    let q = queue_on(&store, 10);
    let issue = |n: usize| {
        q.enqueue_keyed(
            "down",
            &format!("machine/m{n}"),
            CommandKind::DeleteMachine,
            json!(null),
        )
    };
    for _ in 0..5 {
        let tickets: Vec<_> = (0..6).map(issue).collect();
        // Four fit. The rest are refused and leave nothing behind.
        assert_eq!(tickets.iter().filter(|t| t.is_ok()).count(), 4);
        assert!(matches!(tickets[5], Err(RpcError::QueueFull { .. })));
        assert_eq!(records(&store).len(), 4);
        for t in tickets.into_iter().flatten() {
            assert!(matches!(t.wait().await, Err(RpcError::Timeout { .. })));
        }
        // Timed out: no record outlives its command.
        assert!(records(&store).is_empty());
        assert_eq!(q.pending_count(), 0);
        // The dead commands hold their places on the FIFO until the
        // agent polls, so a host that never does is refused from here on.
        assert!(matches!(issue(0), Err(RpcError::QueueFull { .. })));
        assert!(records(&store).is_empty());
        assert!(q.next("down", Duration::ZERO, 16).await.is_empty());
    }
}

#[tokio::test]
async fn a_keyed_command_that_cannot_be_recorded_is_not_queued() {
    let store = Store::in_memory().unwrap();
    let q = queue_on(&store, 60);
    // A server that is not the leader cannot write.
    store.set_write_fence(true);
    let e = q
        .enqueue_keyed("n1", "machine/m1", CommandKind::StartMachine, json!(null))
        .unwrap_err();
    assert!(matches!(e, RpcError::Store(_)), "{e}");
    assert_eq!(q.pending_count(), 0);
    assert!(q.next("n1", Duration::ZERO, 16).await.is_empty());
}

#[tokio::test]
async fn without_a_store_a_keyed_command_still_runs() {
    let q = CommandQueue::new(cfg(60));
    let t = q
        .enqueue_keyed("n1", "machine/m1", CommandKind::StartMachine, json!(null))
        .unwrap();
    let cmds = q.next("n1", Duration::ZERO, 16).await;
    assert_eq!(cmds[0].id, t.id());
    q.ack("n1", CommandResult::ok(t.id())).unwrap();
    assert!(t.wait().await.unwrap().ok);
}
