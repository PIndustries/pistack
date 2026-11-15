// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Draining a host that is up. A machine that cannot be moved keeps
//! running where it is, is reported with a reason an operator can act on,
//! and is tried again by the next call. A failed move never lands the
//! machine in `Error` while it is still running on its old host.

use std::collections::BTreeMap;
use std::time::Duration;

use forge::maintain::{self, Left, Moving};
use forge::{new_host, new_machine, new_size, Host, Machine, MachineSpec, PowerState, Size};
use pistack_rpc::proto::CommandResult;
use pistack_rpc::{CommandQueue, QueueConfig};
use pistack_store::Store;
use pistack_types::{Id, Phase, Resources};

const SIZE: Resources = Resources {
    vcpus: 2,
    memory_mib: 4096,
    disk_gib: 20,
    gpus: 0,
};

fn queue_in(secs: u64) -> CommandQueue {
    CommandQueue::new(QueueConfig {
        command_timeout: Duration::from_secs(secs),
        capacity: 32,
        heartbeat_interval: Duration::from_secs(1),
        ovn: None,
    })
}

fn add_host(store: &Store, name: &str, node: &str, underlay: &str, drivers: &[&str]) -> Host {
    let mut h = new_host(
        name,
        node,
        SIZE,
        drivers.iter().map(|d| (*d).to_string()).collect(),
        BTreeMap::new(),
    );
    h.spec.underlay_ip = Some(underlay.to_owned());
    store.collection::<Host>().create(h).unwrap()
}

/// A host that keeps machine disks where every other host can see them.
fn add_shared_host(
    store: &Store,
    name: &str,
    node: &str,
    underlay: &str,
    drivers: &[&str],
) -> Host {
    let h = add_host(store, name, node, underlay, drivers);
    share_disks(store, &h.meta.id);
    store.collection::<Host>().get(&h.meta.id).unwrap()
}

/// Say this host keeps machine disks on storage every host can see.
fn share_disks(store: &Store, id: &Id) {
    store
        .collection::<Host>()
        .modify(id, |h| {
            h.meta
                .labels
                .insert(forge::DISK_LABEL.into(), forge::DISK_SHARED.into());
            Ok(())
        })
        .unwrap();
}

/// A ready machine placed on `host`, running.
fn running(store: &Store, name: &str, host: &Id) -> Machine {
    let m = store
        .collection::<Machine>()
        .create(new_machine(
            name,
            None,
            MachineSpec {
                size: "m.2x4".into(),
                image: "ubuntu".into(),
                ..Default::default()
            },
        ))
        .unwrap();
    place(store, &m.meta.id, host);
    machine(store, &m.meta.id)
}

fn place(store: &Store, id: &Id, host: &Id) {
    store
        .collection::<Machine>()
        .modify(id, |m| {
            m.status.host = Some(host.clone());
            m.status.phase = Phase::Ready;
            m.status.power = PowerState::On;
            Ok(())
        })
        .unwrap();
}

fn machine(store: &Store, id: &Id) -> Machine {
    store.collection::<Machine>().get(id).unwrap()
}

fn reasons(left: &[Left]) -> Vec<String> {
    left.iter()
        .map(|l| format!("{}: {}", l.name, l.reason))
        .collect()
}

/// Waits for the answer to the migrate command to be folded into status.
async fn settled(store: &Store, id: &Id) -> Machine {
    let mut last = machine(store, id);
    for _ in 0..100 {
        if last.status.phase != Phase::Updating {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        last = machine(store, id);
    }
    last
}

#[tokio::test]
async fn a_refused_migration_leaves_the_machine_running_and_retried() {
    let store = Store::in_memory().unwrap();
    store
        .collection::<Size>()
        .create(new_size("m.2x4", SIZE))
        .unwrap();
    let q = queue_in(20);
    let src = add_shared_host(&store, "src", "node-src", "10.1.0.1", &["kvm", "libvirt"]);
    let dest = add_shared_host(&store, "dest", "node-dest", "10.1.0.2", &["kvm", "libvirt"]);
    let m = running(&store, "web", &src.meta.id);

    let (moving, left) = maintain::drain(&store, Some(&q), Default::default(), &src).unwrap();
    assert!(left.is_empty(), "{:?}", reasons(&left));
    assert_eq!(
        moving,
        vec![Moving {
            id: m.meta.id.clone(),
            name: "web".into(),
            to: dest.meta.id.clone(),
            to_name: "dest".into(),
        }]
    );

    let cmds = q.next("node-src", Duration::from_secs(1), 8).await;
    assert_eq!(cmds.len(), 1);
    q.ack(
        "node-src",
        CommandResult::failed(cmds[0].id, "virsh migrate failed: target is not online"),
    )
    .unwrap();

    // The machine keeps its host, its phase and its power. Only the message
    // says the move did not happen.
    let back = settled(&store, &m.meta.id).await;
    assert_eq!(back.status.phase, Phase::Ready);
    assert_eq!(back.status.host.as_ref(), Some(&src.meta.id));
    assert_eq!(back.status.power, PowerState::On);
    assert!(
        back.status.message.contains("virsh migrate failed"),
        "{}",
        back.status.message
    );

    // The next drain tries the same machine again.
    let src_now = store.collection::<Host>().get(&src.meta.id).unwrap();
    let (moving, left) = maintain::drain(&store, Some(&q), Default::default(), &src_now).unwrap();
    assert!(left.is_empty(), "{:?}", reasons(&left));
    assert_eq!(moving.len(), 1);
    assert_eq!(moving[0].id, m.meta.id);
    assert_eq!(q.pending_count(), 1);
}

#[tokio::test]
async fn an_unanswered_migration_leaves_the_machine_running() {
    let store = Store::in_memory().unwrap();
    store
        .collection::<Size>()
        .create(new_size("m.2x4", SIZE))
        .unwrap();
    let q = queue_in(1);
    let src = add_shared_host(&store, "src", "node-src", "10.1.0.1", &["kvm", "libvirt"]);
    add_host(&store, "dest", "node-dest", "10.1.0.2", &["kvm", "libvirt"]);
    let m = running(&store, "web", &src.meta.id);

    let (moving, left) = maintain::drain(&store, Some(&q), Default::default(), &src).unwrap();
    assert!(left.is_empty(), "{:?}", reasons(&left));
    assert_eq!(moving.len(), 1);
    // Nobody drains the queue, so the command times out.
    let back = settled(&store, &m.meta.id).await;
    assert_eq!(back.status.phase, Phase::Ready);
    assert_eq!(back.status.host.as_ref(), Some(&src.meta.id));

    let src_now = store.collection::<Host>().get(&src.meta.id).unwrap();
    let (moving, left) = maintain::drain(&store, Some(&q), Default::default(), &src_now).unwrap();
    assert_eq!(moving.len(), 1, "{:?}", reasons(&left));
}

/// Nothing can take the machine: the only other host is down. It is not
/// touched, the reason names that host, and the same call moves it once the
/// host is ready again.
#[tokio::test]
async fn a_machine_no_host_can_take_is_left_with_an_actionable_reason() {
    let store = Store::in_memory().unwrap();
    store
        .collection::<Size>()
        .create(new_size("m.2x4", SIZE))
        .unwrap();
    let q = queue_in(20);
    let src = add_shared_host(&store, "src", "node-src", "10.1.0.1", &["kvm", "libvirt"]);
    let mut down = add_shared_host(&store, "down", "node-down", "10.1.0.2", &["kvm", "libvirt"]);
    down.status.phase = Phase::Error;
    down.status.message = "no report for 90s".into();
    let down = store.collection::<Host>().update(down, None).unwrap();
    let m = running(&store, "web", &src.meta.id);

    let (moving, left) = maintain::drain(&store, Some(&q), Default::default(), &src).unwrap();
    assert!(moving.is_empty());
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, m.meta.id);
    assert!(left[0].reason.contains("down"), "{}", left[0].reason);
    assert!(left[0].reason.contains("ready"), "{}", left[0].reason);

    let untouched = machine(&store, &m.meta.id);
    assert_eq!(untouched.status.phase, Phase::Ready);
    assert_eq!(untouched.status.host.as_ref(), Some(&src.meta.id));
    assert!(
        untouched.status.message.is_empty(),
        "{}",
        untouched.status.message
    );
    assert_eq!(q.pending_count(), 0);

    let mut ready = down.clone();
    ready.status.phase = Phase::Ready;
    ready.status.message.clear();
    store.collection::<Host>().update(ready, None).unwrap();
    let (moving, left) = maintain::drain(&store, Some(&q), Default::default(), &src).unwrap();
    assert!(left.is_empty(), "{:?}", reasons(&left));
    assert_eq!(moving.len(), 1);
    assert_eq!(moving[0].to, down.meta.id);
}

/// A host the agent says it cannot live-migrate is reported rather than
/// counted as somewhere the machine can go.
#[tokio::test]
async fn a_destination_without_the_runtime_is_not_counted() {
    let store = Store::in_memory().unwrap();
    store
        .collection::<Size>()
        .create(new_size("m.2x4", SIZE))
        .unwrap();
    let q = queue_in(20);
    let src = add_host(&store, "src", "node-src", "10.1.0.1", &["kvm", "libvirt"]);
    add_host(&store, "plain", "node-plain", "10.1.0.2", &["kvm"]);
    let m = running(&store, "web", &src.meta.id);

    let (moving, left) = maintain::drain(&store, Some(&q), Default::default(), &src).unwrap();
    assert!(moving.is_empty());
    assert_eq!(left.len(), 1, "{:?}", reasons(&left));
    assert_eq!(left[0].id, m.meta.id);
    assert_eq!(q.pending_count(), 0);
}

/// The source host without a live-migration runtime cannot start a move.
#[tokio::test]
async fn a_source_without_the_runtime_keeps_its_machines() {
    let store = Store::in_memory().unwrap();
    store
        .collection::<Size>()
        .create(new_size("m.2x4", SIZE))
        .unwrap();
    let q = queue_in(20);
    let src = add_host(&store, "src", "node-src", "10.1.0.1", &["kvm"]);
    add_shared_host(&store, "dest", "node-dest", "10.1.0.2", &["kvm", "libvirt"]);
    let m = running(&store, "web", &src.meta.id);

    let (moving, left) = maintain::drain(&store, Some(&q), Default::default(), &src).unwrap();
    assert!(moving.is_empty());
    assert_eq!(left.len(), 1);
    assert!(left[0].reason.contains("libvirt"), "{}", left[0].reason);
    let still = machine(&store, &m.meta.id);
    assert_eq!(still.status.phase, Phase::Ready);
    assert_eq!(still.status.host.as_ref(), Some(&src.meta.id));
    assert_eq!(q.pending_count(), 0);
}

/// A machine that is not ready is left alone with a reason that says why.
#[tokio::test]
async fn a_machine_that_is_not_ready_is_left_alone() {
    let store = Store::in_memory().unwrap();
    store
        .collection::<Size>()
        .create(new_size("m.2x4", SIZE))
        .unwrap();
    let q = queue_in(20);
    let src = add_shared_host(&store, "src", "node-src", "10.1.0.1", &["kvm", "libvirt"]);
    add_host(&store, "dest", "node-dest", "10.1.0.2", &["kvm", "libvirt"]);
    let m = running(&store, "web", &src.meta.id);
    store
        .collection::<Machine>()
        .modify(&m.meta.id, |x| {
            x.status.phase = Phase::Error;
            x.status.message = "size 'm.2x4' not found".into();
            Ok(())
        })
        .unwrap();

    let (moving, left) = maintain::drain(&store, Some(&q), Default::default(), &src).unwrap();
    assert!(moving.is_empty());
    assert_eq!(left.len(), 1);
    assert!(left[0].reason.contains("error"), "{}", left[0].reason);
    assert!(
        left[0].reason.contains("size 'm.2x4' not found"),
        "{}",
        left[0].reason
    );
}

/// A machine whose root disk is a file on the host is not asked to move:
/// the hypervisor would refuse it, and the report says so. The same machine
/// is migrated once its host reports the disk directory as shared.
#[tokio::test]
async fn a_machine_with_a_local_root_disk_is_not_asked_to_move() {
    let store = Store::in_memory().unwrap();
    store
        .collection::<Size>()
        .create(new_size("m.2x4", SIZE))
        .unwrap();
    let q = queue_in(20);
    let src = add_host(&store, "src", "node-src", "10.1.0.1", &["kvm", "libvirt"]);
    let dest = add_host(&store, "dest", "node-dest", "10.1.0.2", &["kvm", "libvirt"]);
    let m = running(&store, "web", &src.meta.id);

    let (moving, left) = maintain::drain(&store, Some(&q), Default::default(), &src).unwrap();
    assert!(moving.is_empty());
    assert_eq!(left.len(), 1, "{:?}", reasons(&left));
    assert_eq!(left[0].name, "web");
    assert!(
        left[0].reason.contains("root disk is a file on this host"),
        "{}",
        left[0].reason
    );
    assert!(left[0].reason.contains("share"), "{}", left[0].reason);
    assert_eq!(q.pending_count(), 0);
    let still = machine(&store, &m.meta.id);
    assert_eq!(still.status.phase, Phase::Ready);
    assert_eq!(still.status.host.as_ref(), Some(&src.meta.id));
    assert!(still.status.message.is_empty(), "{}", still.status.message);

    share_disks(&store, &src.meta.id);
    share_disks(&store, &dest.meta.id);
    let src_now = store.collection::<Host>().get(&src.meta.id).unwrap();
    let (moving, left) = maintain::drain(&store, Some(&q), Default::default(), &src_now).unwrap();
    assert!(left.is_empty(), "{:?}", reasons(&left));
    assert_eq!(moving.len(), 1);
    assert_eq!(q.pending_count(), 1);
}

/// A host whose agent says the disk directory is shared takes migrations;
/// one that never said keeps its machines. An agent upgrade alone must not
/// start migrations that end in a refusal.
#[test]
fn a_host_shares_machine_disks_only_when_its_agent_says_so() {
    let store = Store::in_memory().unwrap();
    let mut h = new_host("h", "node-h", SIZE, vec!["libvirt".into()], BTreeMap::new());
    let m = new_machine("m", None, MachineSpec::default());
    assert!(!forge::machine_disks_are_shared(&h));
    assert!(maintain::disk_holds_machine(&store, &m, &h).is_some());
    h.meta
        .labels
        .insert("pistack.io/disk".into(), "local".into());
    assert!(maintain::disk_holds_machine(&store, &m, &h).is_some());
    h.meta
        .labels
        .insert("pistack.io/disk".into(), "shared".into());
    assert!(forge::machine_disks_are_shared(&h));
    assert!(maintain::disk_holds_machine(&store, &m, &h).is_none());
}

#[test]
fn a_host_can_live_migrate_when_its_agent_reports_the_runtime() {
    let mut h = new_host("h", "node-h", SIZE, vec!["kvm".into()], BTreeMap::new());
    assert!(!maintain::can_live_migrate(&h));
    h.spec.drivers.push(maintain::LIVE_MIGRATION_DRIVER.into());
    assert!(maintain::can_live_migrate(&h));
}
