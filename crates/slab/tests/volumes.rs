// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Integration: in-memory store + local file backend in a tempdir.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use pistack_rpc::{
    AgentHello, CommandKind, CommandQueue, CommandResult, QueueConfig, VolumeAttach, VolumeDetach,
};
use pistack_types::{kind, Caller, Id, Phase, Resource, Spec};
use serde_json::{json, Value};
use slab::{
    Client, Create, LocalFileBackend, Protocol, SnapshotSpec, VolumeSource, VolumeSpec,
    VolumeTypeSpec,
};
use std::time::Duration;
use tower::ServiceExt;

const GIB: u64 = 1024 * 1024 * 1024;

fn client(dir: &tempfile::TempDir) -> Client {
    let c = Client::local(dir.path()).expect("client");
    c.seed_defaults().expect("seed");
    c
}

fn create(name: &str, spec: VolumeSpec) -> Create<VolumeSpec> {
    Create {
        name: name.into(),
        description: String::new(),
        labels: Default::default(),
        annotations: Default::default(),
        project: Some(Id::new("proj")),
        spec,
    }
}

fn spec(size_gib: u64) -> VolumeSpec {
    VolumeSpec {
        size_gib,
        volume_type: "standard".into(),
        source: VolumeSource::None,
        bootable: false,
        encrypted: false,
    }
}

#[tokio::test]
async fn create_snapshot_delete_on_local_backend() {
    let dir = tempfile::tempdir().expect("tempdir");
    let c = client(&dir);
    let backend = LocalFileBackend::new(dir.path());

    // create 1 GiB volume -> Pending
    let vol = c
        .create_volume(create("data", spec(1)))
        .await
        .expect("create");
    assert_eq!(vol.status.phase, Phase::Pending);
    assert_eq!(vol.spec.volume_type, "standard");

    // reconcile -> Ready, sparse file of exactly 1 GiB
    let vol = c
        .reconcile_volume(&vol.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    assert_eq!(vol.status.phase, Phase::Ready);
    let path = backend.volume_path(&vol);
    assert_eq!(
        vol.status.backend_ref.as_deref(),
        Some(path.to_string_lossy().as_ref())
    );
    let md = std::fs::metadata(&path).expect("volume file exists");
    assert_eq!(md.len(), GIB);

    // attach info for forge
    let info = c
        .attach_info(&vol.meta.id, "host-a")
        .await
        .expect("attach_info");
    assert_eq!(info.protocol, Protocol::Local);

    // extend to 2 GiB
    let vol = c.extend_volume(&vol.meta.id, 2).await.expect("extend");
    assert_eq!(vol.status.phase, Phase::Updating);
    let vol = c
        .reconcile_volume(&vol.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    assert_eq!(vol.status.phase, Phase::Ready);
    assert_eq!(std::fs::metadata(&path).expect("file").len(), 2 * GIB);

    // snapshot
    let snap = c
        .create_snapshot(Create {
            name: "data-snap".into(),
            description: String::new(),
            labels: Default::default(),
            annotations: Default::default(),
            project: None,
            spec: SnapshotSpec {
                volume: vol.meta.id.clone(),
            },
        })
        .await
        .expect("snapshot");
    assert_eq!(
        snap.meta.project, vol.meta.project,
        "snapshot inherits project"
    );
    let snap = c
        .reconcile_snapshot(&snap.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    assert_eq!(snap.status.phase, Phase::Ready);
    assert_eq!(snap.status.size_gib, 2);
    let snap_path = backend.snapshot_path(&snap);
    assert!(snap_path.exists());

    // volume with snapshots cannot be deleted
    assert_eq!(
        c.request_delete_volume(&vol.meta.id)
            .expect_err("has snapshots")
            .code,
        pistack_types::error::ErrorCode::Conflict
    );

    // restore the snapshot into a new, bigger volume
    let restored = c
        .create_volume(create(
            "restored",
            VolumeSpec {
                size_gib: 3,
                source: VolumeSource::Snapshot(snap.meta.id.clone()),
                ..spec(3)
            },
        ))
        .await
        .expect("create from snapshot");
    let restored = c
        .reconcile_volume(&restored.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    assert_eq!(restored.status.phase, Phase::Ready);
    assert_eq!(
        std::fs::metadata(backend.volume_path(&restored))
            .expect("file")
            .len(),
        3 * GIB
    );

    // delete snapshot, then volume; files and records go away
    c.request_delete_snapshot(&snap.meta.id)
        .expect("request delete");
    assert!(c
        .reconcile_snapshot(&snap.meta.id)
        .await
        .expect("reconcile")
        .is_none());
    assert!(!snap_path.exists());

    c.request_delete_volume(&vol.meta.id)
        .expect("request delete");
    assert!(c
        .reconcile_volume(&vol.meta.id)
        .await
        .expect("reconcile")
        .is_none());
    assert!(!path.exists());
    assert!(c.volumes().try_get(&vol.meta.id).expect("store").is_none());
}

fn attached(v: &slab::Volume) -> (bool, &str, &str) {
    let c = v
        .status
        .conditions
        .iter()
        .find(|c| c.kind == "Attached")
        .expect("Attached condition");
    (c.status, c.reason.as_str(), c.message.as_str())
}

#[tokio::test]
async fn attach_is_ready_only_when_the_host_path_is_real() {
    let dir = tempfile::tempdir().expect("tempdir");
    let c = client(&dir);
    let vol = c.create_volume(create("a", spec(1))).await.expect("create");
    let machine = Id::new("mach");
    let machine_id = machine.to_string();

    assert!(c
        .begin_attach(&vol.meta.id, &machine, Some("vdb".into()))
        .is_err());
    let vol = c
        .reconcile_volume(&vol.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    let real = LocalFileBackend::new(dir.path()).volume_path(&vol);
    assert!(real.is_file());

    let v = c
        .begin_attach(&vol.meta.id, &machine, Some("vdb".into()))
        .expect("attach");
    assert_eq!(v.status.attached_to.as_ref(), Some(&machine));
    assert_eq!(v.status.device.as_deref(), Some("vdb"));
    assert!(v.status.host_path.is_none());
    assert_eq!(attached(&v), (false, "Attaching", machine_id.as_str()));
    assert!(v.is_attached_ready().is_none());

    // A second attach is rejected while the first is still Attaching.
    assert!(c
        .begin_attach(&vol.meta.id, &machine, Some("vdb".into()))
        .is_err());
    assert!(c
        .begin_attach(&vol.meta.id, &Id::new("mach"), None)
        .is_err());
    let v = c.get_volume(None, &vol.meta.id.to_string()).expect("get");
    assert_eq!(v.status.attached_to.as_ref(), Some(&machine));
    assert_eq!(attached(&v).1, "Attaching");
    assert!(c.request_delete_volume(&vol.meta.id).is_err());

    let missing = dir.path().join("missing-disk");
    let err = c
        .confirm_attach(
            &vol.meta.id,
            &machine,
            Some("vdb".into()),
            Some(missing.to_str().expect("utf8")),
        )
        .expect_err("missing path");
    assert!(err.message.contains("does not exist"), "{err}");
    let v = c.get_volume(None, &vol.meta.id.to_string()).expect("get");
    assert!(v.status.attached_to.is_none());
    assert!(v.status.host_path.is_none());
    assert!(!attached(&v).0);
    assert_eq!(attached(&v).1, "AttachFailed");
    assert!(attached(&v).2.contains("does not exist"));
    assert!(v.is_attached_ready().is_none());

    c.begin_attach(&vol.meta.id, &machine, Some("vdb".into()))
        .expect("attach again");
    let v = c
        .confirm_attach(
            &vol.meta.id,
            &machine,
            Some("vdb".into()),
            Some(real.to_str().expect("utf8")),
        )
        .expect("confirm");
    assert_eq!(attached(&v), (true, "attached", machine_id.as_str()));
    assert_eq!(v.status.host_path.as_deref(), Some(real.to_str().unwrap()));
    assert_eq!(v.status.device.as_deref(), Some("vdb"));
    assert_eq!(v.is_attached_ready(), Some(real.as_path()));
    // Same machine stays attached. A different machine is still refused.
    c.begin_attach(&vol.meta.id, &machine, Some("vdb".into()))
        .expect("idempotent");
    assert!(c
        .begin_attach(&vol.meta.id, &Id::new("mach"), None)
        .is_err());

    let v = c.mark_detached(&vol.meta.id).expect("detach");
    assert!(v.status.attached_to.is_none());
    assert!(v.status.host_path.is_none());
    assert!(v.status.device.is_none());
    assert_eq!(attached(&v), (false, "detached", ""));
    assert!(v.is_attached_ready().is_none());
    c.request_delete_volume(&vol.meta.id)
        .expect("delete after detach");
}

#[tokio::test]
async fn rbd_network_disk_confirms_without_a_host_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let c = client(&dir);
    c.create_volume_type(Create {
        name: "ceph".into(),
        description: String::new(),
        labels: Default::default(),
        annotations: Default::default(),
        project: None,
        spec: VolumeTypeSpec {
            backend: "rbd".into(),
            params: Default::default(),
        },
    })
    .expect("type");
    let mut spec = spec(1);
    spec.volume_type = "ceph".into();
    let vol = c.create_volume(create("img", spec)).await.expect("create");
    // Ready without calling rbd. This test only covers the attach record.
    c.volumes()
        .modify(&vol.meta.id, |v| {
            v.status.phase = Phase::Ready;
            Ok(())
        })
        .expect("ready");
    let machine = Id::new("mach");
    c.begin_attach(&vol.meta.id, &machine, Some("vdb".into()))
        .expect("attach");
    let v = c
        .confirm_attach(&vol.meta.id, &machine, Some("vdb".into()), None)
        .expect("network disk");
    assert!(attached(&v).0);
    assert!(v.status.host_path.is_none());
    assert!(v.is_attached_ready().is_none());

    c.mark_detached(&vol.meta.id).expect("detach");
    c.begin_attach(&vol.meta.id, &machine, Some("vdb".into()))
        .expect("attach");
    let missing = dir.path().join("no-rbd");
    let err = c
        .confirm_attach(
            &vol.meta.id,
            &machine,
            Some("vdb".into()),
            Some(missing.to_str().expect("utf8")),
        )
        .expect_err("missing mapped device");
    assert!(err.message.contains("does not exist"), "{err}");
    let v = c.get_volume(None, &vol.meta.id.to_string()).expect("get");
    assert!(v.status.attached_to.is_none());
    assert_eq!(attached(&v).1, "AttachFailed");
}

#[tokio::test]
async fn stale_machine_attach_is_cleared() {
    let dir = tempfile::tempdir().expect("tempdir");
    let c = client(&dir);
    let vol = c.create_volume(create("a", spec(1))).await.expect("create");
    let vol = c
        .reconcile_volume(&vol.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    let real = LocalFileBackend::new(dir.path()).volume_path(&vol);
    let machine = place_machine(&c, "fleet");
    c.begin_attach(&vol.meta.id, &machine, Some("vdb".into()))
        .expect("attach");
    let v = c
        .confirm_attach(
            &vol.meta.id,
            &machine,
            Some("vdb".into()),
            Some(real.to_str().expect("utf8")),
        )
        .expect("confirm");
    assert_eq!(v.status.attach_node.as_deref(), Some("fleet"));
    c.reconcile_stale_attaches().await.expect("reconcile");
    let v = c.get_volume(None, &vol.meta.id.to_string()).expect("get");
    assert_eq!(v.status.attached_to.as_ref(), Some(&machine));
    assert!(v.is_attached_ready().is_some());

    delete_machine(&c, &machine);
    c.reconcile_stale_attaches().await.expect("reconcile");
    let v = c.get_volume(None, &vol.meta.id.to_string()).expect("get");
    assert!(v.status.attached_to.is_none());
    assert!(v.status.host_path.is_none());
    assert!(v.status.attach_node.is_none());
    assert_eq!(attached(&v), (false, "detached", ""));
}

#[tokio::test]
async fn stale_rbd_map_on_a_live_agent_is_unmapped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let queue = CommandQueue::new(QueueConfig {
        command_timeout: Duration::from_secs(5),
        ..QueueConfig::default()
    });
    let c = client(&dir).with_queue(queue.clone());
    let vol = c.create_volume(create("a", spec(1))).await.expect("create");
    let vol = c
        .reconcile_volume(&vol.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    let real = LocalFileBackend::new(dir.path()).volume_path(&vol);
    let machine = place_machine(&c, "fleet");
    c.begin_attach(&vol.meta.id, &machine, Some("vdb".into()))
        .expect("attach");
    c.confirm_attach(
        &vol.meta.id,
        &machine,
        Some("vdb".into()),
        Some(real.to_str().expect("utf8")),
    )
    .expect("confirm");
    // The map path is what the agent recorded. The volume file stands in
    // for the confirmed attach; the stale pass keys off the rbd device path.
    c.volumes()
        .modify(&vol.meta.id, |v| {
            v.status.host_path = Some("/dev/rbd0".into());
            Ok(())
        })
        .expect("record map");
    delete_machine(&c, &machine);

    // No hello yet: the agent is gone, so status is cleared without a command.
    c.reconcile_stale_attaches().await.expect("reconcile");
    assert_eq!(queue.pending_count(), 0);
    let v = c.get_volume(None, &vol.meta.id.to_string()).expect("get");
    assert!(v.status.attached_to.is_none());

    // Put the claim back, announce the agent, and require an unmap.
    c.volumes()
        .modify(&vol.meta.id, |v| {
            v.status.attached_to = Some(machine.clone());
            v.status.host_path = Some("/dev/rbd0".into());
            v.status.attach_node = Some("fleet".into());
            v.status.device = Some("vdb".into());
            Ok(())
        })
        .expect("claim");
    queue.hello(&AgentHello {
        node_name: "fleet".into(),
        capabilities: Vec::new(),
        resources: Default::default(),
        version: String::new(),
        labels: Default::default(),
        underlay_ip: None,
    });
    let agent = tokio::spawn({
        let queue = queue.clone();
        async move {
            let cmds = queue.next("fleet", Duration::from_secs(5), 1).await;
            let cmd = cmds.into_iter().next().expect("detach command");
            assert_eq!(cmd.kind, CommandKind::DetachVolume);
            let payload = cmd.payload.clone();
            queue
                .ack("fleet", CommandResult::ok(cmd.id))
                .expect("ack detach");
            payload
        }
    });
    c.reconcile_stale_attaches().await.expect("reconcile");
    let payload = agent.await.expect("agent");
    let detach: VolumeDetach = serde_json::from_value(payload).expect("detach payload");
    assert_eq!(detach.path, "/dev/rbd0");
    assert_eq!(detach.volume, vol.meta.id);
    assert!(detach.target.is_none());
    let v = c.get_volume(None, &vol.meta.id.to_string()).expect("get");
    assert!(v.status.attached_to.is_none());
    assert!(v.status.host_path.is_none());
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
struct HostSpec {
    node_name: String,
}

impl Spec for HostSpec {
    const KIND: &'static str = kind::HOST;
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
struct HostStatus {}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
struct MachineSpec {}

impl Spec for MachineSpec {
    const KIND: &'static str = kind::MACHINE;
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
struct MachineStatus {
    #[serde(default)]
    host: Option<Id>,
}

fn place_machine(c: &Client, node: &str) -> Id {
    let host = c
        .store()
        .collection::<Resource<HostSpec, HostStatus>>()
        .create(Resource::new(
            node,
            None,
            HostSpec {
                node_name: node.into(),
            },
        ))
        .expect("host");
    let mut machine = Resource::<MachineSpec, MachineStatus>::new(
        "guest",
        Some(Id::new(kind::PROJECT)),
        MachineSpec {},
    );
    machine.status.host = Some(host.meta.id);
    c.store()
        .collection::<Resource<MachineSpec, MachineStatus>>()
        .create(machine)
        .expect("machine")
        .meta
        .id
}

fn delete_machine(c: &Client, id: &Id) {
    c.store()
        .collection::<Resource<MachineSpec, MachineStatus>>()
        .delete(id)
        .expect("delete machine");
}

#[tokio::test]
async fn local_file_attach_and_detach_use_the_agent_queue() {
    let dir = tempfile::tempdir().expect("tempdir");
    let queue = CommandQueue::new(QueueConfig {
        command_timeout: Duration::from_secs(5),
        ..QueueConfig::default()
    });
    let c = client(&dir).with_queue(queue.clone());
    let vol = c
        .create_volume(create("disk", spec(1)))
        .await
        .expect("create");
    let vol = c
        .reconcile_volume(&vol.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    let path = LocalFileBackend::new(dir.path())
        .volume_path(&vol)
        .to_string_lossy()
        .into_owned();
    let machine = place_machine(&c, "fleet");

    let agent = tokio::spawn({
        let queue = queue.clone();
        async move {
            let cmds = queue.next("fleet", Duration::from_secs(5), 1).await;
            let cmd = cmds.into_iter().next().expect("attach command");
            assert_eq!(cmd.kind, CommandKind::AttachVolume);
            let payload = cmd.payload.clone();
            queue
                .ack(
                    "fleet",
                    CommandResult::ok(cmd.id).with_payload(json!({"device": "vdb"})),
                )
                .expect("ack attach");
            payload
        }
    });
    let attached = c
        .plug_volume(&vol.meta.id, &machine, Some("vdb".into()))
        .await
        .expect("plug");
    assert_eq!(attached.device, "vdb");
    assert!(attached.host_path.is_none());
    let payload = agent.await.expect("agent");
    let attach: VolumeAttach = serde_json::from_value(payload).expect("attach payload");
    assert_eq!(attach.machine, machine);
    assert_eq!(attach.volume, vol.meta.id);
    assert_eq!(attach.path, path);
    assert!(attach.target.is_none());
    assert_eq!(attach.device.as_deref(), Some("vdb"));
    assert!(!attach.read_only);

    let agent = tokio::spawn({
        let queue = queue.clone();
        async move {
            let cmds = queue.next("fleet", Duration::from_secs(5), 1).await;
            let cmd = cmds.into_iter().next().expect("detach command");
            assert_eq!(cmd.kind, CommandKind::DetachVolume);
            let payload = cmd.payload.clone();
            queue
                .ack("fleet", CommandResult::ok(cmd.id))
                .expect("ack detach");
            payload
        }
    });
    c.unplug_volume(&vol.meta.id, &machine, Some("vdb".into()))
        .await
        .expect("unplug");
    let payload = agent.await.expect("agent");
    let detach: VolumeDetach = serde_json::from_value(payload).expect("detach payload");
    assert_eq!(detach.path, path);
    assert!(detach.target.is_none());
    assert_eq!(detach.device.as_deref(), Some("vdb"));
    assert_eq!(detach.volume, vol.meta.id);
}

#[tokio::test]
async fn agent_confirm_sets_attached_and_a_failure_clears_the_claim() {
    let dir = tempfile::tempdir().expect("tempdir");
    let queue = CommandQueue::new(QueueConfig {
        command_timeout: Duration::from_secs(5),
        ..QueueConfig::default()
    });
    let c = client(&dir).with_queue(queue.clone());
    let vol = c
        .create_volume(create("disk", spec(1)))
        .await
        .expect("create");
    let vol = c
        .reconcile_volume(&vol.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    let path = LocalFileBackend::new(dir.path())
        .volume_path(&vol)
        .to_string_lossy()
        .into_owned();
    let machine = place_machine(&c, "fleet");
    let machine_id = machine.to_string();

    let agent = tokio::spawn({
        let queue = queue.clone();
        async move {
            let cmds = queue.next("fleet", Duration::from_secs(5), 1).await;
            let cmd = cmds.into_iter().next().expect("attach command");
            queue
                .ack(
                    "fleet",
                    CommandResult::failed(cmd.id, "host path /data/disk does not exist"),
                )
                .expect("ack failure");
        }
    });
    let err = c
        .attach_volume(&vol.meta.id, &machine, Some("vdb".into()))
        .await
        .expect_err("agent failure");
    agent.await.expect("agent");
    assert!(err.message.contains("does not exist"), "{err}");
    let v = c.get_volume(None, &vol.meta.id.to_string()).expect("get");
    assert!(v.status.attached_to.is_none());
    assert_eq!(attached(&v).1, "AttachFailed");
    assert!(attached(&v).2.contains("does not exist"));

    let agent = tokio::spawn({
        let queue = queue.clone();
        let path = path.clone();
        async move {
            let cmds = queue.next("fleet", Duration::from_secs(5), 1).await;
            let cmd = cmds.into_iter().next().expect("attach command");
            queue
                .ack(
                    "fleet",
                    CommandResult::ok(cmd.id).with_payload(json!({
                        "device": "vdb",
                        "host_path": path,
                    })),
                )
                .expect("ack attach");
        }
    });
    let v = c
        .attach_volume(&vol.meta.id, &machine, Some("vdb".into()))
        .await
        .expect("attach");
    agent.await.expect("agent");
    assert_eq!(attached(&v), (true, "attached", machine_id.as_str()));
    assert_eq!(v.status.host_path.as_deref(), Some(path.as_str()));
    assert_eq!(
        v.is_attached_ready().map(std::path::Path::to_path_buf),
        Some(std::path::PathBuf::from(&path))
    );
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(v) => {
            req = req.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let mut req = req.body(body).expect("request");
    req.extensions_mut().insert(admin_caller());
    let resp = app.clone().oneshot(req).await.expect("response");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.expect("body");
    let v = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json")
    };
    (status, v)
}

#[tokio::test]
async fn http_crud_with_stale_revision() {
    let dir = tempfile::tempdir().expect("tempdir");
    let c = client(&dir);
    let app = slab::http::router(c.clone());

    // volume types: seeded "standard"
    let (st, v) = call(&app, "GET", "/volume-types", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["total"], 1);
    assert_eq!(v["items"][0]["name"], "standard");
    assert_eq!(v["items"][0]["spec"]["backend"], "local");

    // create
    let (st, vol) = call(
        &app,
        "POST",
        "/volumes",
        Some(json!({ "name": "web-data", "labels": { "tier": "web" }, "spec": { "size_gib": 1 } })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{vol}");
    let id = vol["id"].as_str().expect("id").to_owned();
    assert!(id.starts_with("vol-"));
    assert_eq!(vol["status"]["phase"], "pending");
    assert_eq!(vol["revision"], 1);

    // duplicate name -> 409
    let (st, _) = call(
        &app,
        "POST",
        "/volumes",
        Some(json!({ "name": "web-data", "spec": { "size_gib": 1 } })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);

    // bad size -> 400; unknown type -> 400
    let (st, _) = call(
        &app,
        "POST",
        "/volumes",
        Some(json!({ "name": "z", "spec": { "size_gib": 0 } })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = call(
        &app,
        "POST",
        "/volumes",
        Some(json!({ "name": "z", "spec": { "size_gib": 1, "volume_type": "gold" } })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // list, by label, by name
    let (_, page) = call(&app, "GET", "/volumes", None).await;
    assert_eq!(page["total"], 1);
    let (_, page) = call(&app, "GET", "/volumes?labels=tier=db", None).await;
    assert_eq!(page["total"], 0);
    let (st, got) = call(&app, "GET", "/volumes/web-data", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got["id"], id);

    // update with If-Match: correct, then stale -> 409
    let resp = app
        .clone()
        .oneshot(with_admin(
            Request::builder()
                .method("PUT")
                .uri(format!("/volumes/{id}"))
                .header("content-type", "application/json")
                .header("if-match", "1")
                .body(Body::from(
                    json!({ "description": "web node data" }).to_string(),
                ))
                .expect("req"),
        ))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .clone()
        .oneshot(with_admin(
            Request::builder()
                .method("PUT")
                .uri(format!("/volumes/{id}"))
                .header("content-type", "application/json")
                .header("if-match", "1")
                .body(Body::from(json!({ "description": "stale" }).to_string()))
                .expect("req"),
        ))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // extend before ready -> 409; make ready; extend -> 202
    let (st, _) = call(
        &app,
        "POST",
        &format!("/volumes/{id}/extend"),
        Some(json!({ "size_gib": 2 })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    let vid = Id::parse("vol", &id).expect("id");
    c.reconcile_volume(&vid).await.expect("reconcile");
    let (st, v) = call(
        &app,
        "POST",
        &format!("/volumes/{id}/extend"),
        Some(json!({ "size_gib": 2 })),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{v}");
    c.reconcile_volume(&vid).await.expect("reconcile");

    // attach / detach
    let machine = Id::new("mach").to_string();
    let (st, v) = call(
        &app,
        "POST",
        &format!("/volumes/{id}/attach"),
        Some(json!({ "machine": machine, "device": "vdb" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["status"]["attached_to"], machine);
    let cond = v["status"]["conditions"]
        .as_array()
        .expect("conditions")
        .iter()
        .find(|c| c["type"] == "Attached")
        .expect("Attached");
    assert_eq!(cond["status"], false);
    assert_eq!(cond["reason"], "Attaching");
    assert!(v["status"].get("host_path").is_none());
    let (st, _) = call(&app, "DELETE", &format!("/volumes/{id}"), None).await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "attached volumes cannot be deleted"
    );
    let (st, _) = call(&app, "POST", &format!("/volumes/{id}/detach"), None).await;
    assert_eq!(st, StatusCode::OK);

    // snapshots and backups
    let (st, snap) = call(
        &app,
        "POST",
        "/snapshots",
        Some(json!({ "name": "s1", "spec": { "volume": id } })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{snap}");
    let (st, bak) = call(
        &app,
        "POST",
        "/backups",
        Some(json!({ "name": "b1", "spec": { "volume": id, "target": "backups" } })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{bak}");
    let bid = bak["id"].as_str().expect("id");
    let (st, body) = call(&app, "POST", &format!("/backups/{bid}/restore"), None).await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert_ne!(bak["status"]["phase"], "ready");
    let (st, _) = call(&app, "DELETE", &format!("/backups/{bid}"), None).await;
    assert_eq!(st, StatusCode::ACCEPTED);

    let sid = snap["id"].as_str().expect("id");
    let (st, _) = call(&app, "DELETE", &format!("/snapshots/{sid}"), None).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    c.reconcile_snapshot(&Id::parse("snap", sid).expect("id"))
        .await
        .expect("reconcile");

    // delete volume -> 202, reconcile removes it -> 404
    let (st, _) = call(&app, "DELETE", &format!("/volumes/{id}"), None).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    c.reconcile_volume(&vid).await.expect("reconcile");
    let (st, _) = call(&app, "GET", &format!("/volumes/{id}"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // volume type in use cannot be deleted; unused one can
    let (st, vt) = call(
        &app,
        "POST",
        "/volume-types",
        Some(
            json!({ "name": "gold", "spec": { "backend": "local", "params": { "tier": "nvme" } } }),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{vt}");
    let (st, _) = call(&app, "DELETE", "/volume-types/gold", None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn reconciler_loop_drives_volume_to_ready() {
    let dir = tempfile::tempdir().expect("tempdir");
    let c = client(&dir);
    let shutdown = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(slab::reconcile::run(
        c.clone(),
        c.store().events(),
        shutdown.clone(),
        pistack_types::Leadership::always(),
    ));

    let vol = c
        .create_volume(create("looped", spec(1)))
        .await
        .expect("create");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let cur = c.volumes().get(&vol.meta.id).expect("get");
        if cur.status.phase == Phase::Ready {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "volume never became ready: {cur:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    shutdown.cancel();
    task.await.expect("reconciler exits");
}

fn type_create(
    name: &str,
    backend: &str,
    params: serde_json::Value,
) -> Create<slab::VolumeTypeSpec> {
    let mut map = std::collections::BTreeMap::new();
    if let Some(obj) = params.as_object() {
        for (k, v) in obj {
            map.insert(k.clone(), v.as_str().unwrap_or_default().to_owned());
        }
    }
    Create {
        name: name.into(),
        description: String::new(),
        labels: Default::default(),
        annotations: Default::default(),
        project: None,
        spec: slab::VolumeTypeSpec {
            backend: backend.into(),
            params: map,
        },
    }
}

fn make_bucket(c: &Client, project: &Id, name: &str) {
    silo::Client::new(c.store().clone(), c.silo_root())
        .create_bucket(
            pistack_types::Meta::new(kind::BUCKET, name, Some(project.clone())),
            silo::BucketSpec::default(),
        )
        .expect("bucket");
}

#[tokio::test]
async fn empty_backend_uses_local_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let c = client(&dir);
    c.create_volume_type(type_create("blank", "", json!({})))
        .expect("type");
    let vt = c.get_volume_type("blank").expect("type");
    assert_eq!(vt.spec.backend, "local");
    let vol = c
        .create_volume(create(
            "blank-vol",
            VolumeSpec {
                volume_type: "blank".into(),
                ..spec(1)
            },
        ))
        .await
        .expect("volume");
    let vol = c
        .reconcile_volume(&vol.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    assert_eq!(vol.status.phase, Phase::Ready);
    assert!(LocalFileBackend::new(dir.path()).volume_path(&vol).exists());
}

#[tokio::test]
async fn lvm_and_rbd_types_stay_unavailable_without_pool() {
    let dir = tempfile::tempdir().expect("tempdir");
    let c = client(&dir);
    let lvm_log = dir.path().join("lvm.log");
    let lvm_bin = dir.path().join("lvm");
    let quoted = lvm_log.display().to_string().replace('\'', "'\\''");
    std::fs::write(
        &lvm_bin,
        format!("#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{quoted}'\nexit 1\n"),
    )
    .unwrap();
    let rbd_log = dir.path().join("rbd.log");
    let rbd_bin = dir.path().join("rbd");
    let rbd_quoted = rbd_log.display().to_string().replace('\'', "'\\''");
    std::fs::write(
        &rbd_bin,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{rbd_quoted}'\nif [ \"$1\" = \"pool\" ]; then exit 1; fi\nexit 0\n"
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&lvm_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(&rbd_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("PISTACK_LVM_BIN", &lvm_bin);
    std::env::set_var("PISTACK_RBD_BIN", &rbd_bin);
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            std::env::remove_var("PISTACK_LVM_BIN");
            std::env::remove_var("PISTACK_RBD_BIN");
        }
    }
    let _reset = Reset;

    c.create_volume_type(type_create(
        "thin",
        "lvm",
        json!({"vg": "pistack-absent-vg", "pool": "pistack-absent-pool"}),
    ))
    .expect("lvm type");
    let lvm = c
        .create_volume(create(
            "lvm-vol",
            VolumeSpec {
                volume_type: "thin".into(),
                ..spec(1)
            },
        ))
        .await
        .expect("lvm volume");
    let lvm = c
        .reconcile_volume(&lvm.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    assert_ne!(lvm.status.phase, Phase::Ready);
    assert!(lvm.status.backend_ref.is_none());
    assert!(!LocalFileBackend::new(dir.path()).volume_path(&lvm).exists());
    let logged = std::fs::read_to_string(&lvm_log).unwrap_or_default();
    assert!(!logged.contains("--thin"), "{logged}");
    assert!(!logged.contains("--virtualsize"), "{logged}");

    c.create_volume_type(type_create(
        "ceph",
        "rbd",
        json!({"pool": "volumes", "monitors": "10.0.0.1:6789"}),
    ))
    .expect("rbd type");
    let rbd = c
        .create_volume(create(
            "rbd-vol",
            VolumeSpec {
                volume_type: "ceph".into(),
                ..spec(1)
            },
        ))
        .await
        .expect("rbd volume");
    let rbd = c
        .reconcile_volume(&rbd.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    assert_ne!(rbd.status.phase, Phase::Ready);
    assert!(rbd.status.backend_ref.is_none());
    let logged = std::fs::read_to_string(&rbd_log).unwrap_or_default();
    assert!(!logged.lines().any(|line| line == "create"), "{logged}");

    c.create_volume_type(type_create("nomon", "rbd", json!({"pool": "volumes"})))
        .expect("rbd type without monitors");
    let quiet = dir.path().join("rbd-quiet.log");
    let quiet_bin = dir.path().join("rbd-quiet");
    let quiet_quoted = quiet.display().to_string().replace('\'', "'\\''");
    std::fs::write(
        &quiet_bin,
        format!("#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{quiet_quoted}'\nexit 0\n"),
    )
    .unwrap();
    std::fs::set_permissions(&quiet_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("PISTACK_RBD_BIN", &quiet_bin);
    let bare = c
        .create_volume(create(
            "rbd-bare",
            VolumeSpec {
                volume_type: "nomon".into(),
                ..spec(1)
            },
        ))
        .await
        .expect("volume");
    let bare = c
        .reconcile_volume(&bare.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    assert_ne!(bare.status.phase, Phase::Ready);
    assert!(!quiet.exists(), "missing monitors must not run rbd");
}

#[tokio::test]
async fn backup_round_trip_and_oversize_refusal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let c = client(&dir);
    let vol = c
        .create_volume(create("src", spec(1)))
        .await
        .expect("volume");
    let vol = c
        .reconcile_volume(&vol.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    let path = LocalFileBackend::new(dir.path()).volume_path(&vol);
    let payload = b"pistack-backup-payload";
    std::fs::write(&path, payload).expect("shrink source");
    let project = vol.meta.project.clone().expect("project");
    make_bucket(&c, &project, "backups");

    let bak = c
        .create_backup(Create {
            name: "nightly".into(),
            description: String::new(),
            labels: Default::default(),
            annotations: Default::default(),
            project: None,
            spec: slab::BackupSpec {
                volume: vol.meta.id.clone(),
                target: "backups".into(),
            },
        })
        .await
        .expect("backup");
    assert_eq!(bak.status.phase, Phase::Pending);
    let bak = c
        .reconcile_backup(&bak.meta.id)
        .await
        .expect("upload")
        .expect("exists");
    assert_eq!(bak.status.phase, Phase::Ready, "{bak:?}");
    let key = bak.status.object_ref.clone().expect("object");
    let bucket = silo::Client::new(c.store().clone(), c.silo_root())
        .get_bucket(Some(&project), "backups")
        .expect("bucket");
    let (obj, bytes) = silo::Client::new(c.store().clone(), c.silo_root())
        .get_object(&bucket.meta.id, &key)
        .await
        .expect("object bytes");
    assert_eq!(bytes, payload);
    assert_eq!(obj.status.size_bytes, payload.len() as u64);

    let restored = c.restore_backup(&bak.meta.id).await.expect("restore");
    assert_eq!(restored.status.phase, Phase::Ready);
    assert_ne!(restored.meta.id, vol.meta.id);
    let restored_path = LocalFileBackend::new(dir.path()).volume_path(&restored);
    let mut got = vec![0u8; payload.len()];
    std::io::Read::read_exact(&mut std::fs::File::open(&restored_path).unwrap(), &mut got).unwrap();
    assert_eq!(got, payload);

    let huge = silo::MAX_OBJECT_BYTES as u64 + 1;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(huge)
        .unwrap();
    let big = c
        .create_backup(Create {
            name: "too-big".into(),
            description: String::new(),
            labels: Default::default(),
            annotations: Default::default(),
            project: None,
            spec: slab::BackupSpec {
                volume: vol.meta.id.clone(),
                target: "backups".into(),
            },
        })
        .await
        .expect("big backup");
    let err = c
        .reconcile_backup(&big.meta.id)
        .await
        .expect_err("oversize");
    assert_eq!(
        err.code,
        pistack_types::error::ErrorCode::Unavailable,
        "{err}"
    );
    assert!(err.message.contains(&huge.to_string()), "{err}");
    let big = c.backups().get(&big.meta.id).expect("big");
    assert_ne!(big.status.phase, Phase::Ready);
    assert!(big.status.object_ref.is_none());
    let objects = silo::Client::new(c.store().clone(), c.silo_root())
        .list_objects(&bucket.meta.id)
        .expect("list");
    assert!(
        objects.iter().all(|o| o.spec.key == key),
        "oversize backup must not write an object: {objects:?}"
    );

    let blocked = dir.path().join("not-a-dir");
    std::fs::write(&blocked, b"x").unwrap();
    let c = Client::local(dir.path())
        .expect("client")
        .with_silo_root(&blocked);
    c.seed_defaults().expect("seed");
    let vol = c
        .create_volume(create("src2", spec(1)))
        .await
        .expect("volume");
    let vol = c
        .reconcile_volume(&vol.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    let path = LocalFileBackend::new(dir.path()).volume_path(&vol);
    std::fs::write(&path, b"nope").unwrap();
    let project = vol.meta.project.clone().expect("project");
    make_bucket(&c, &project, "backups");
    let bak = c
        .create_backup(Create {
            name: "broken".into(),
            description: String::new(),
            labels: Default::default(),
            annotations: Default::default(),
            project: None,
            spec: slab::BackupSpec {
                volume: vol.meta.id.clone(),
                target: "backups".into(),
            },
        })
        .await
        .expect("backup");
    let bak = c
        .reconcile_backup(&bak.meta.id)
        .await
        .expect("failed write still returns the record")
        .expect("exists");
    assert_ne!(bak.status.phase, Phase::Ready);
    assert!(bak.status.object_ref.is_none(), "{bak:?}");
}

#[tokio::test]
async fn refs_outside_the_slab_dir_are_not_removed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("tempdir");
    let sentinel = outside.path().join("sentinel");
    std::fs::write(&sentinel, b"keep").unwrap();
    let c = client(&dir);

    let vol = c
        .create_volume(create("data", spec(1)))
        .await
        .expect("volume");
    let vol = c
        .reconcile_volume(&vol.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    let snap = c
        .create_snapshot(Create {
            name: "data-snap".into(),
            description: String::new(),
            labels: Default::default(),
            annotations: Default::default(),
            project: None,
            spec: SnapshotSpec {
                volume: vol.meta.id.clone(),
            },
        })
        .await
        .expect("snapshot");
    let snap = c
        .reconcile_snapshot(&snap.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");

    c.snapshots()
        .modify(&snap.meta.id, |s| {
            s.status.backend_ref = Some(sentinel.display().to_string());
            s.status.phase = Phase::Deleting;
            Ok(())
        })
        .expect("modify snapshot");
    let snap = c
        .reconcile_snapshot(&snap.meta.id)
        .await
        .expect("reconcile")
        .expect("snapshot record stays");
    assert_eq!(snap.status.phase, Phase::Error);
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"keep");

    c.volumes()
        .modify(&vol.meta.id, |v| {
            v.status.backend_ref = Some(sentinel.display().to_string());
            v.status.phase = Phase::Deleting;
            Ok(())
        })
        .expect("modify volume");
    let vol = c
        .reconcile_volume(&vol.meta.id)
        .await
        .expect("reconcile")
        .expect("volume record stays");
    assert_eq!(vol.status.phase, Phase::Error);
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"keep");
}

fn admin_caller() -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "dev-admin".into(),
        project: None,
        roles: vec!["admin".into()],
        system: true,
    }
}

fn with_admin(mut req: Request<Body>) -> Request<Body> {
    req.extensions_mut().insert(admin_caller());
    req
}

#[tokio::test]
async fn missing_caller_is_unauthorized() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = slab::http::router(client(&dir));
    let res = app
        .oneshot(
            Request::builder()
                .uri("/volumes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "unauthorized");
    assert_eq!(body["error"]["message"], "no credentials");
}
