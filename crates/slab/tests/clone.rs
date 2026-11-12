// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Integration: a volume made from a snapshot or from another volume on the
//! LVM and RBD backends, with stand-in `lvm` and `rbd` binaries that record
//! each command. This file is its own test binary because it sets the
//! process-wide `PISTACK_LVM_BIN` and `PISTACK_RBD_BIN`.

use pistack_types::{Id, Phase};
use slab::{Client, Create, Snapshot, SnapshotSpec, Volume, VolumeSource, VolumeSpec};
use std::path::{Path, PathBuf};

fn create<S>(name: &str, project: Option<Id>, spec: S) -> Create<S> {
    Create {
        name: name.into(),
        description: String::new(),
        labels: Default::default(),
        annotations: Default::default(),
        project,
        spec,
    }
}

/// A script that records each invocation as one line and succeeds.
fn stand_in(dir: &Path, name: &str) -> (PathBuf, PathBuf) {
    let log = dir.join(format!("{name}.log"));
    let bin = dir.join(name);
    let quoted = log.display().to_string().replace('\'', "'\\''");
    std::fs::write(
        &bin,
        format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{quoted}'\nexit 0\n"),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    (bin, log)
}

/// Commands recorded since the last call, without the availability probes.
fn issued(log: &Path) -> Vec<String> {
    let logged = std::fs::read_to_string(log).unwrap_or_default();
    let _ = std::fs::remove_file(log);
    logged
        .lines()
        .filter(|line| !line.starts_with("--noheadings") && !line.starts_with("pool stats"))
        .map(str::to_owned)
        .collect()
}

async fn ready_volume(c: &Client, name: &str, spec: VolumeSpec) -> Volume {
    let vol = c
        .create_volume(create(name, Some(Id::new("proj")), spec))
        .await
        .expect("create volume");
    let vol = c
        .reconcile_volume(&vol.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    assert_eq!(vol.status.phase, Phase::Ready, "{:?}", vol.status);
    vol
}

async fn ready_snapshot(c: &Client, name: &str, vol: &Volume) -> Snapshot {
    let snap = c
        .create_snapshot(create(
            name,
            None,
            SnapshotSpec {
                volume: vol.meta.id.clone(),
            },
        ))
        .await
        .expect("create snapshot");
    let snap = c
        .reconcile_snapshot(&snap.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    assert_eq!(snap.status.phase, Phase::Ready, "{:?}", snap.status);
    snap
}

fn spec(volume_type: &str, size_gib: u64, source: VolumeSource) -> VolumeSpec {
    VolumeSpec {
        size_gib,
        volume_type: volume_type.into(),
        source,
        bootable: false,
        encrypted: false,
    }
}

fn volume_type(name: &str, backend: &str, params: &[(&str, &str)]) -> Create<slab::VolumeTypeSpec> {
    create(
        name,
        None,
        slab::VolumeTypeSpec {
            backend: backend.into(),
            params: params
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        },
    )
}

#[tokio::test]
async fn snapshot_and_volume_sources_reach_lvm_and_rbd() {
    let dir = tempfile::tempdir().expect("tempdir");
    let c = Client::local(dir.path()).expect("client");
    c.seed_defaults().expect("seed");
    let (lvm_bin, lvm_log) = stand_in(dir.path(), "lvm");
    let (rbd_bin, rbd_log) = stand_in(dir.path(), "rbd");
    std::env::set_var("PISTACK_LVM_BIN", &lvm_bin);
    std::env::set_var("PISTACK_RBD_BIN", &rbd_bin);

    // ---- lvm ----
    c.create_volume_type(volume_type(
        "thin",
        "lvm",
        &[("vg", "vg0"), ("pool", "pistack-thin")],
    ))
    .expect("lvm type");
    let src = ready_volume(&c, "lvm-src", spec("thin", 2, VolumeSource::None)).await;
    let src_id = &src.meta.id;
    let snap = ready_snapshot(&c, "lvm-snap", &src).await;
    let snap_id = &snap.meta.id;
    issued(&lvm_log);

    let restored = ready_volume(
        &c,
        "lvm-restored",
        spec("thin", 3, VolumeSource::Snapshot(snap_id.clone())),
    )
    .await;
    let id = &restored.meta.id;
    assert_eq!(
        restored.status.backend_ref.as_deref(),
        Some(format!("/dev/vg0/{id}").as_str())
    );
    assert_eq!(
        issued(&lvm_log),
        vec![
            format!("--snapshot --name {id} vg0/{snap_id}"),
            format!("-ay -K vg0/{id}"),
            format!("--size 3G vg0/{id}"),
        ]
    );

    let cloned = ready_volume(
        &c,
        "lvm-cloned",
        spec("thin", 2, VolumeSource::Volume(src_id.clone())),
    )
    .await;
    let id = &cloned.meta.id;
    assert_eq!(
        issued(&lvm_log),
        vec![
            format!("--snapshot --name {id} vg0/{src_id}"),
            format!("-ay -K vg0/{id}"),
        ]
    );

    // ---- rbd ----
    c.create_volume_type(volume_type(
        "ceph",
        "rbd",
        &[("pool", "volumes"), ("monitors", "10.0.0.1:6789")],
    ))
    .expect("rbd type");
    let src = ready_volume(&c, "rbd-src", spec("ceph", 2, VolumeSource::None)).await;
    let src_id = &src.meta.id;
    let snap = ready_snapshot(&c, "rbd-snap", &src).await;
    let snap_id = &snap.meta.id;
    issued(&rbd_log);

    let restored = ready_volume(
        &c,
        "rbd-restored",
        spec("ceph", 2, VolumeSource::Snapshot(snap_id.clone())),
    )
    .await;
    let id = &restored.meta.id;
    assert_eq!(
        restored.status.backend_ref.as_deref(),
        Some(format!("volumes/{id}").as_str())
    );
    assert_eq!(
        issued(&rbd_log),
        vec![format!("clone volumes/{src_id}@{snap_id} volumes/{id}")]
    );

    let cloned = ready_volume(
        &c,
        "rbd-cloned",
        spec("ceph", 3, VolumeSource::Volume(src_id.clone())),
    )
    .await;
    let id = &cloned.meta.id;
    assert_eq!(
        issued(&rbd_log),
        vec![
            format!("cp --image-feature layering,exclusive-lock volumes/{src_id} volumes/{id}"),
            format!("resize --size 3G volumes/{id}"),
        ]
    );

    // A source that is gone fails the create. It does not fall back to a
    // blank volume.
    let doomed = c
        .create_volume(create(
            "rbd-doomed",
            Some(Id::new("proj")),
            spec("ceph", 2, VolumeSource::Snapshot(snap_id.clone())),
        ))
        .await
        .expect("create volume");
    c.request_delete_snapshot(snap_id).expect("request delete");
    assert!(c
        .reconcile_snapshot(snap_id)
        .await
        .expect("reconcile")
        .is_none());
    issued(&rbd_log);
    let doomed = c
        .reconcile_volume(&doomed.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    assert_eq!(doomed.status.phase, Phase::Error);
    assert!(doomed.status.backend_ref.is_none());
    assert_eq!(issued(&rbd_log), Vec::<String>::new());
}
