// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A machine is evacuated off a dead host and starts on another one. Its
//! RBD volume is plugged in there; its local volume is not.
//!
//! This file is its own test binary because it points `PISTACK_RBD_BIN` at
//! a stand-in for the whole process.

use pistack_rpc::{
    AgentHello, CommandKind, CommandQueue, CommandResult, DiskTarget, QueueConfig, VolumeAttach,
    VolumeAttached,
};
use pistack_types::{kind, Id, Phase, Resource, Spec};
use slab::{Client, Create, VolumeSource, VolumeSpec, VolumeTypeSpec};
use std::time::Duration;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
struct HostSpec {
    node_name: String,
}

impl Spec for HostSpec {
    const KIND: &'static str = kind::HOST;
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
struct MachineSpec {}

impl Spec for MachineSpec {
    const KIND: &'static str = kind::MACHINE;
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
struct MachineStatus {
    #[serde(default)]
    host: Option<Id>,
    #[serde(default)]
    phase: Phase,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
struct Nothing {}

type Host = Resource<HostSpec, Nothing>;
type Machine = Resource<MachineSpec, MachineStatus>;

fn host(c: &Client, node: &str) -> Id {
    c.store()
        .collection::<Host>()
        .create(Resource::new(
            node,
            None,
            HostSpec {
                node_name: node.into(),
            },
        ))
        .expect("host")
        .meta
        .id
}

async fn volume(c: &Client, name: &str, volume_type: &str) -> Id {
    let vol = c
        .create_volume(Create {
            name: name.into(),
            description: String::new(),
            labels: Default::default(),
            annotations: Default::default(),
            project: Some(Id::new("proj")),
            spec: VolumeSpec {
                size_gib: 1,
                volume_type: volume_type.into(),
                source: VolumeSource::None,
                bootable: false,
                encrypted: false,
            },
        })
        .await
        .expect("create");
    // Ready without a backend call. This test is about the attach record.
    c.volumes()
        .modify(&vol.meta.id, |v| {
            v.status.phase = Phase::Ready;
            Ok(())
        })
        .expect("ready");
    vol.meta.id
}

/// Record `vol` as attached to `machine` on the node the machine is on.
fn attached(c: &Client, vol: &Id, machine: &Id, host_path: Option<&str>) {
    c.begin_attach(vol, machine, Some("vdb".into()))
        .expect("attach");
    c.volumes()
        .modify(vol, |v| {
            v.status.host_path = host_path.map(str::to_owned);
            v.status.conditions.clear();
            v.status.conditions.push(pistack_types::Condition {
                kind: "Attached".into(),
                status: true,
                reason: "attached".into(),
                message: machine.to_string(),
                last_transition: chrono::Utc::now(),
            });
            Ok(())
        })
        .expect("confirm");
}

fn place(c: &Client, machine: &Id, host: Option<&Id>, phase: Phase, from: Option<&Id>) {
    c.store()
        .collection::<Machine>()
        .modify(machine, |m| {
            m.status.host = host.cloned();
            m.status.phase = phase;
            if let Some(from) = from {
                m.meta
                    .annotations
                    .insert("pistack.io/evacuated-from".into(), from.to_string());
            }
            Ok(())
        })
        .expect("place");
}

#[tokio::test]
async fn an_rbd_volume_follows_an_evacuated_machine_and_a_local_one_does_not() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Stands in for `rbd`: the pool is there.
    let rbd = dir.path().join("rbd");
    std::fs::write(&rbd, "#!/bin/sh\nexit 0\n").expect("write");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&rbd, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    std::env::set_var("PISTACK_RBD_BIN", &rbd);

    let queue = CommandQueue::new(QueueConfig {
        command_timeout: Duration::from_secs(5),
        ..QueueConfig::default()
    });
    let c = Client::local(dir.path())
        .expect("client")
        .with_queue(queue.clone());
    c.seed_defaults().expect("seed");
    c.create_volume_type(Create {
        name: "ceph".into(),
        description: String::new(),
        labels: Default::default(),
        annotations: Default::default(),
        project: None,
        spec: VolumeTypeSpec {
            backend: "rbd".into(),
            params: [
                ("pool".to_owned(), "volumes".to_owned()),
                ("monitors".to_owned(), "10.0.0.1:6789".to_owned()),
            ]
            .into_iter()
            .collect(),
        },
    })
    .expect("type");

    let dead = host(&c, "dead");
    let spare = host(&c, "spare");
    for node in ["dead", "spare"] {
        queue.hello(&AgentHello {
            node_name: node.into(),
            capabilities: Vec::new(),
            resources: Default::default(),
            version: String::new(),
            labels: Default::default(),
            underlay_ip: None,
        });
    }
    let mut guest = Machine::new("guest", Some(Id::new(kind::PROJECT)), MachineSpec {});
    guest.status.host = Some(dead.clone());
    guest.status.phase = Phase::Ready;
    let guest = c
        .store()
        .collection::<Machine>()
        .create(guest)
        .expect("machine")
        .meta
        .id;
    let shared = volume(&c, "shared", "ceph").await;
    let local = volume(&c, "local", "standard").await;
    attached(&c, &shared, &guest, None);
    attached(&c, &local, &guest, Some("/var/lib/pistack/volumes/local"));
    let node_of = |vol: &Id| c.volumes().get(vol).expect("volume").status.attach_node;
    assert_eq!(node_of(&shared).as_deref(), Some("dead"));

    // A machine that was not evacuated is not touched, wherever it is.
    c.reattach_evacuated(None).await.expect("pass");
    place(&c, &guest, Some(&spare), Phase::Ready, None);
    c.reattach_evacuated(None).await.expect("pass");
    assert_eq!(queue.pending_count(), 0);

    // Evacuated and waiting for a host, then starting: nothing to plug into.
    place(&c, &guest, None, Phase::Pending, Some(&dead));
    c.reattach_evacuated(None).await.expect("pass");
    place(&c, &guest, Some(&spare), Phase::Creating, None);
    c.reattach_evacuated(Some(&guest)).await.expect("pass");
    assert_eq!(queue.pending_count(), 0);

    // Running on the new host: the RBD image is plugged in there.
    place(&c, &guest, Some(&spare), Phase::Ready, None);
    let agent = tokio::spawn({
        let queue = queue.clone();
        async move {
            let cmds = queue.next("spare", Duration::from_secs(5), 4).await;
            assert_eq!(cmds.len(), 1, "one attach, for the shared volume only");
            let cmd = cmds.into_iter().next().expect("attach command");
            assert_eq!(cmd.kind, CommandKind::AttachVolume);
            let done = VolumeAttached {
                device: "vdb".into(),
                host_path: None,
            };
            queue
                .ack(
                    "spare",
                    CommandResult::ok(cmd.id).with_payload(serde_json::to_value(done).unwrap()),
                )
                .expect("ack");
            cmd.payload
        }
    });
    c.reattach_evacuated(Some(&guest)).await.expect("pass");
    let req: VolumeAttach = serde_json::from_value(agent.await.expect("agent")).expect("payload");
    assert_eq!(req.volume, shared);
    assert_eq!(req.machine, guest);
    assert_eq!(req.device.as_deref(), Some("vdb"));
    match req.target {
        Some(DiskTarget::Rbd { pool, image, .. }) => {
            assert_eq!(pool, "volumes");
            assert_eq!(image, shared.to_string());
        }
        other => panic!("expected an rbd target, got {other:?}"),
    }
    let vol = c.volumes().get(&shared).expect("volume");
    assert_eq!(vol.status.attach_node.as_deref(), Some("spare"));
    assert_eq!(vol.status.attached_to.as_ref(), Some(&guest));
    assert!(vol.status.host_path.is_none());

    // The local volume stays recorded on the dead host, and a later pass
    // has nothing left to do.
    assert_eq!(node_of(&local).as_deref(), Some("dead"));
    c.reattach_evacuated(None).await.expect("pass");
    assert_eq!(queue.pending_count(), 0);
    assert!(queue
        .next("dead", Duration::from_millis(50), 4)
        .await
        .is_empty());
}
