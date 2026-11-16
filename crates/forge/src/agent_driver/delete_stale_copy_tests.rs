// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::registrar::Registrar;
use super::test_support::*;
use crate::driver::DriverRegistry;
use crate::types::{Machine, EVACUATED_FROM_ANNOTATION};
use pistack_rpc::proto::{CommandKind, CommandResult, MachineRef};
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use pistack_types::Id;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn a_returning_host_is_told_to_delete_the_machines_evacuated_off_it() {
    let store = Store::in_memory().unwrap();
    let queue = CommandQueue::default();
    let reg = Registrar::new(
        store.clone(),
        Arc::new(DriverRegistry::new()),
        queue.clone(),
    );
    let old = reg.on_hello(&hello("old")).unwrap();
    let new = reg.on_hello(&hello("new")).unwrap();
    let col = store.collection::<Machine>();

    // Evacuated off `old` and running on `new`.
    let mut moved = placed("moved", &new.meta.id, &[]);
    moved
        .meta
        .annotations
        .insert(EVACUATED_FROM_ANNOTATION.into(), old.meta.id.to_string());
    let moved = col.create(moved).unwrap();
    // Evacuated off `old` and still waiting for a host.
    let mut waiting = machine("waiting");
    waiting
        .meta
        .annotations
        .insert(EVACUATED_FROM_ANNOTATION.into(), old.meta.id.to_string());
    let waiting = col.create(waiting).unwrap();
    // On `new` and never evacuated. `old` reporting it proves nothing.
    let other = col.create(placed("other", &new.meta.id, &[])).unwrap();
    // Really on `old`.
    let own = col.create(placed("own", &old.meta.id, &[])).unwrap();

    let beat = reports(
        "old",
        &[
            &moved.meta.id,
            &waiting.meta.id,
            &other.meta.id,
            &own.meta.id,
        ],
    );
    reg.on_heartbeat(&beat).unwrap();
    let cmds = queue.next("old", Duration::from_secs(1), 8).await;
    let mut deleted: Vec<Id> = cmds
        .iter()
        .map(|c| {
            assert_eq!(c.kind, CommandKind::DeleteMachine);
            serde_json::from_value::<MachineRef>(c.payload.clone())
                .unwrap()
                .id
        })
        .collect();
    deleted.sort();
    let mut want = vec![moved.meta.id.clone(), waiting.meta.id.clone()];
    want.sort();
    assert_eq!(deleted, want);
    // The new host is never asked to delete its copy.
    reg.on_heartbeat(&reports("new", &[&moved.meta.id, &other.meta.id]))
        .unwrap();
    assert!(queue
        .next("new", Duration::from_millis(50), 8)
        .await
        .is_empty());

    // The host heartbeats again before it answered: nothing is queued twice.
    reg.on_heartbeat(&beat).unwrap();
    assert!(queue
        .next("old", Duration::from_millis(50), 8)
        .await
        .is_empty());

    // A delete that failed is asked for again while the host still
    // reports the machine. One that worked is not: the host stops
    // reporting it.
    for c in &cmds {
        let failed = serde_json::from_value::<MachineRef>(c.payload.clone())
            .unwrap()
            .id
            == moved.meta.id;
        let result = if failed {
            CommandResult::failed(c.id, "domain is busy")
        } else {
            CommandResult::ok(c.id)
        };
        queue.ack("old", result).unwrap();
    }
    let still = reports("old", &[&moved.meta.id, &own.meta.id]);
    let retry = loop {
        reg.on_heartbeat(&still).unwrap();
        let cmds = queue.next("old", Duration::from_millis(50), 8).await;
        if !cmds.is_empty() {
            break cmds;
        }
    };
    assert_eq!(retry.len(), 1);
    assert_eq!(
        serde_json::from_value::<MachineRef>(retry[0].payload.clone())
            .unwrap()
            .id,
        moved.meta.id
    );
    queue.ack("old", CommandResult::ok(retry[0].id)).unwrap();

    // Nothing in the store moved.
    assert_eq!(
        col.get(&moved.meta.id).unwrap().status.host.as_ref(),
        Some(&new.meta.id)
    );
    assert_eq!(
        col.get(&own.meta.id).unwrap().status.host.as_ref(),
        Some(&old.meta.id)
    );
}
