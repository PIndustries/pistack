// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::begin_migration::begin_migration;
use super::migrate_dest::migrate_dest;
use super::test_support::*;
use crate::types::{Host, Machine};
use pistack_rpc::proto::{CommandKind, CommandResult};
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use pistack_types::Phase;
use std::time::Duration;

#[tokio::test]
async fn migrate_refuses_a_host_without_underlay_and_enqueues_when_present() {
    use pistack_rpc::proto::MigrateRequest;

    let store = Store::in_memory().unwrap();
    let queue = CommandQueue::default();
    let hosts = store.collection::<Host>();
    let src = hosts
        .create(host_with("src", "node-src", None, Phase::Ready))
        .unwrap();
    let bare = hosts
        .create(host_with("bare", "node-bare", None, Phase::Ready))
        .unwrap();
    let down = hosts
        .create(host_with(
            "down",
            "node-down",
            Some("10.9.0.1"),
            Phase::Error,
        ))
        .unwrap();
    let dest = hosts
        .create(host_with(
            "dest",
            "node-dest",
            Some("10.1.0.2"),
            Phase::Ready,
        ))
        .unwrap();
    assert!(migrate_dest(&bare).is_err());
    assert_eq!(migrate_dest(&dest).unwrap(), "qemu+ssh://10.1.0.2/system");

    let machines = store.collection::<Machine>();
    let named = machines
        .create(placed("web", &src.meta.id, &["tenant"]))
        .unwrap();
    let quiet = machines.create(placed("quiet", &src.meta.id, &[])).unwrap();

    let err = begin_migration(&store, &queue, &named, &bare).unwrap_err();
    assert!(err.message.contains("underlay"));
    let still = machines.get(&named.meta.id).unwrap();
    assert_eq!(still.status.host.as_ref(), Some(&src.meta.id));
    assert_eq!(queue.pending_count(), 0);

    let err = begin_migration(&store, &queue, &quiet, &bare).unwrap_err();
    assert!(err.message.contains("underlay"));
    assert_eq!(
        machines.get(&quiet.meta.id).unwrap().status.host.as_ref(),
        Some(&src.meta.id)
    );

    let err = begin_migration(&store, &queue, &named, &src).unwrap_err();
    assert!(err.message.contains("current host"));
    let err = begin_migration(&store, &queue, &named, &down).unwrap_err();
    assert!(err.message.contains("not ready"));
    let mut loose = machine("loose");
    loose.status.phase = Phase::Ready;
    let loose = machines.create(loose).unwrap();
    assert!(begin_migration(&store, &queue, &loose, &dest).is_err());
    assert_eq!(queue.pending_count(), 0);
    assert_eq!(
        machines.get(&named.meta.id).unwrap().status.host.as_ref(),
        Some(&src.meta.id)
    );

    let moved = begin_migration(&store, &queue, &named, &dest).unwrap();
    assert_eq!(moved.status.host.as_ref(), Some(&src.meta.id));
    assert_eq!(moved.status.phase, Phase::Updating);
    assert_eq!(moved.status.message, format!("migrating {}", dest.meta.id));
    let cmds = queue.next("node-src", Duration::from_secs(1), 8).await;
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].kind, CommandKind::MigrateMachine);
    assert_eq!(cmds[0].kind.to_string(), "migrate_machine");
    let req: MigrateRequest = serde_json::from_value(cmds[0].payload.clone()).unwrap();
    assert_eq!(req.id, named.meta.id);
    assert_eq!(req.dest, "qemu+ssh://10.1.0.2/system");

    queue
        .ack(
            "node-src",
            CommandResult::failed(cmds[0].id, "virsh migrate failed"),
        )
        .unwrap();
    // The guest was running before the command and a refusal changes
    // nothing on the host, so the machine is ready where it has always
    // been. Only the message says the move did not happen.
    let mut failed = None;
    for _ in 0..50 {
        let cur = machines.get(&named.meta.id).unwrap();
        if cur.status.phase != Phase::Updating {
            failed = Some(cur);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let failed = failed.expect("a failed migrate leaves the machine on its host");
    assert_eq!(failed.status.phase, Phase::Ready);
    assert_eq!(failed.status.host.as_ref(), Some(&src.meta.id));
    assert!(failed.status.message.contains("virsh migrate failed"));
}

/// A root disk that is a file on the host cannot follow the machine, so
/// the hypervisor would refuse the move. The call says so itself and the
/// machine is not touched.
#[tokio::test]
async fn migrate_refuses_a_machine_whose_disk_is_local_to_its_host() {
    let store = Store::in_memory().unwrap();
    let queue = CommandQueue::default();
    let hosts = store.collection::<Host>();
    // `host_with` says its disks are shared; this host does not say.
    let mut src = host_with("src", "node-src", Some("10.1.0.1"), Phase::Ready);
    src.meta.labels.remove(crate::types::DISK_LABEL);
    let src = hosts.create(src).unwrap();
    let dest = hosts
        .create(host_with(
            "dest",
            "node-dest",
            Some("10.1.0.2"),
            Phase::Ready,
        ))
        .unwrap();
    let machines = store.collection::<Machine>();
    let m = machines.create(placed("web", &src.meta.id, &[])).unwrap();

    let err = begin_migration(&store, &queue, &m, &dest).unwrap_err();
    assert_eq!(
        err.code.status(),
        axum::http::StatusCode::UNPROCESSABLE_ENTITY
    );
    assert!(
        err.message.contains("root disk is a file on this host"),
        "{}",
        err.message
    );
    assert!(err.message.contains("share"), "{}", err.message);
    assert_eq!(queue.pending_count(), 0);
    let still = machines.get(&m.meta.id).unwrap();
    assert_eq!(still.status.phase, Phase::Ready);
    assert_eq!(still.status.host.as_ref(), Some(&src.meta.id));
    assert!(still.status.message.is_empty(), "{}", still.status.message);
}
