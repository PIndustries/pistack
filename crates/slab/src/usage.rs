// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! What a project uses in slab: `volumes`, `volumes_gib`, `snapshots`,
//! `backups` and `backups_gib`. It is counted from the rows each time the
//! quota hook is asked, and copied to the project for reading.
//!
//! A volume, snapshot or backup stops counting when its delete is
//! accepted, before the reconciler has removed it. A backup counts the
//! size of the volume it was taken from, which is known when it is asked
//! for; the bytes it ends up holding are not known until it is written.

use crate::types::{Backup, Snapshot, Volume};
use gate::quota::{self, BACKUPS, BACKUPS_GIB, SNAPSHOTS, VOLUMES, VOLUMES_GIB};
use pistack_store::Store;
use pistack_types::{ApiResult, Id, Phase};

/// Set the first time a delete is accepted. A delete that then fails
/// leaves the volume in `Error`, and it must not count again.
const RELEASED: &str = "pistack.io/volume-quota-released";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub volumes: u64,
    pub volumes_gib: u64,
    pub snapshots: u64,
    pub backups: u64,
    pub backups_gib: u64,
}

impl Usage {
    /// Usage of a gate quota resource; `0` for one slab does not own.
    pub fn of(&self, resource: &str) -> u64 {
        match resource {
            VOLUMES => self.volumes,
            VOLUMES_GIB => self.volumes_gib,
            SNAPSHOTS => self.snapshots,
            BACKUPS => self.backups,
            BACKUPS_GIB => self.backups_gib,
            _ => 0,
        }
    }
}

pub fn count(store: &Store, project: &Id) -> ApiResult<Usage> {
    let mut usage = Usage::default();
    for v in store.collection::<Volume>().list(Some(project))? {
        if v.status.phase == Phase::Deleting || v.meta.annotations.contains_key(RELEASED) {
            continue;
        }
        usage.volumes += 1;
        usage.volumes_gib += v.spec.size_gib;
    }
    for s in store.collection::<Snapshot>().list(Some(project))? {
        if s.status.phase != Phase::Deleting {
            usage.snapshots += 1;
        }
    }
    for b in store.collection::<Backup>().list(Some(project))? {
        if b.status.phase != Phase::Deleting {
            usage.backups += 1;
            usage.backups_gib += b.status.size_gib;
        }
    }
    Ok(usage)
}

/// Mark `volume` deleting. True only the first time, when it stops counting.
pub fn claim_release(volume: &mut Volume) -> bool {
    let first =
        volume.status.phase != Phase::Deleting && !volume.meta.annotations.contains_key(RELEASED);
    volume.status.phase = Phase::Deleting;
    if first {
        volume
            .meta
            .annotations
            .insert(RELEASED.to_owned(), "true".to_owned());
    }
    first
}

/// Count again and copy the result to the project. For a caller that
/// holds gate's admission lock.
pub(crate) fn mirror(store: &Store, project: Option<&Id>) {
    let Some(project) = project else {
        return;
    };
    match count(store, project) {
        Ok(u) => quota::mirror_usage(
            store,
            project,
            &[
                (VOLUMES, u.volumes),
                (VOLUMES_GIB, u.volumes_gib),
                (SNAPSHOTS, u.snapshots),
                (BACKUPS, u.backups),
                (BACKUPS_GIB, u.backups_gib),
            ],
        ),
        Err(e) => tracing::warn!(%project, error = %e, "slab: usage was not recorded"),
    }
}

/// [`mirror`] for a caller that does not hold the lock, after a delete.
pub(crate) fn refresh(store: &Store, project: Option<&Id>) {
    if project.is_none() {
        return;
    }
    let _admission = quota::admission();
    mirror(store, project);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Create;
    use crate::quota::AllowAll;
    use crate::types::{BackupSpec, SnapshotSpec, VolumeSource, VolumeSpec};
    use crate::Client;
    use gate::{Project, ProjectSpec, ProjectStatus};
    use pistack_types::error::ErrorCode;
    use pistack_types::{kind, ApiError, Meta};
    use std::sync::Arc;

    fn tenant(store: &Store, name: &str, limits: &[(&str, u64)]) -> Id {
        let mut spec = ProjectSpec::default();
        for (r, n) in limits {
            spec.quotas.insert((*r).into(), *n);
        }
        store
            .collection::<Project>()
            .create(Project {
                meta: Meta::new(kind::PROJECT, name, None),
                spec,
                status: ProjectStatus::default(),
            })
            .unwrap()
            .meta
            .id
    }

    fn used(store: &Store, project: &Id, resource: &str) -> u64 {
        store
            .collection::<Project>()
            .get(project)
            .unwrap()
            .status
            .usage
            .get(resource)
            .copied()
            .unwrap_or(0)
    }

    fn client() -> (tempfile::TempDir, Client) {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::local(dir.path()).unwrap();
        client.seed_defaults().unwrap();
        (dir, client)
    }

    fn body<S>(project: &Id, name: &str, spec: S) -> Create<S> {
        Create {
            name: name.into(),
            description: String::new(),
            labels: Default::default(),
            annotations: Default::default(),
            project: Some(project.clone()),
            spec,
        }
    }

    async fn create(client: &Client, project: &Id, name: &str, gib: u64) -> ApiResult<Volume> {
        let spec = VolumeSpec {
            size_gib: gib,
            volume_type: "standard".into(),
            source: VolumeSource::None,
            bootable: false,
            encrypted: false,
        };
        client.create_volume(body(project, name, spec)).await
    }

    /// A volume the reconciler has made ready, as a snapshot or backup needs.
    async fn ready(client: &Client, project: &Id, name: &str, gib: u64) -> Volume {
        let vol = create(client, project, name, gib).await.unwrap();
        let vol = client
            .reconcile_volume(&vol.meta.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(vol.status.phase, Phase::Ready);
        vol
    }

    async fn snapshot(client: &Client, vol: &Volume, name: &str) -> ApiResult<Snapshot> {
        let project = vol.meta.project.as_ref().unwrap();
        let spec = SnapshotSpec {
            volume: vol.meta.id.clone(),
        };
        client.create_snapshot(body(project, name, spec)).await
    }

    async fn backup(client: &Client, vol: &Volume, name: &str) -> ApiResult<Backup> {
        let project = vol.meta.project.as_ref().unwrap();
        let spec = BackupSpec {
            volume: vol.meta.id.clone(),
            target: "backups".into(),
        };
        client.create_backup(body(project, name, spec)).await
    }

    fn refused(e: &ApiError, resource: &str, used: u64, asked: u64, limit: u64) {
        assert_eq!(e.code, ErrorCode::QuotaExceeded);
        assert_eq!(
            e.message,
            format!(
                "quota exceeded for {resource} in project 'tight': \
                 {used} used + {asked} requested > {limit}"
            )
        );
        assert_eq!(e.details["resource"], resource);
        assert_eq!(e.details["limit"], limit);
        assert_eq!(e.details["used"], used);
        assert_eq!(e.details["requested"], asked);
    }

    #[tokio::test]
    async fn a_zero_volume_quota_refuses_and_leaves_no_row() {
        let (_dir, client) = client();
        let project = tenant(client.store(), "tight", &[(VOLUMES, 0)]);
        let err = create(&client, &project, "data", 1).await.unwrap_err();
        refused(&err, VOLUMES, 0, 1, 0);
        assert!(client.volumes().list(Some(&project)).unwrap().is_empty());
        assert_eq!(used(client.store(), &project, VOLUMES), 0);
    }

    #[tokio::test]
    async fn no_volume_quota_creates_and_delete_releases_once() {
        let (_dir, client) = client();
        let project = tenant(client.store(), "open", &[]);
        let vol = create(&client, &project, "data", 1).await.unwrap();
        assert_eq!(client.volumes().list(Some(&project)).unwrap().len(), 1);
        assert_eq!(used(client.store(), &project, VOLUMES), 1);

        client.request_delete_volume(&vol.meta.id).unwrap();
        assert_eq!(used(client.store(), &project, VOLUMES), 0);
        client.request_delete_volume(&vol.meta.id).unwrap();
        assert_eq!(used(client.store(), &project, VOLUMES), 0);
        assert_eq!(client.volumes().list(Some(&project)).unwrap().len(), 1);

        let again = create(&client, &project, "other", 1).await.unwrap();
        assert_eq!(used(client.store(), &project, VOLUMES), 1);
        client.request_delete_volume(&again.meta.id).unwrap();
        assert_eq!(used(client.store(), &project, VOLUMES), 0);
    }

    #[tokio::test]
    async fn volume_gigabytes_are_limited_in_total() {
        let (_dir, client) = client();
        let project = tenant(client.store(), "tight", &[(VOLUMES_GIB, 10)]);
        let first = create(&client, &project, "a", 6).await.unwrap();
        create(&client, &project, "b", 4).await.unwrap();
        assert_eq!(used(client.store(), &project, VOLUMES_GIB), 10);
        let err = create(&client, &project, "c", 1).await.unwrap_err();
        refused(&err, VOLUMES_GIB, 10, 1, 10);
        assert_eq!(client.volumes().list(Some(&project)).unwrap().len(), 2);

        client.request_delete_volume(&first.meta.id).unwrap();
        assert_eq!(used(client.store(), &project, VOLUMES_GIB), 4);
        create(&client, &project, "c", 6).await.unwrap();
        assert_eq!(used(client.store(), &project, VOLUMES_GIB), 10);

        let open = tenant(client.store(), "open", &[]);
        create(&client, &open, "huge", 4096).await.unwrap();
        assert_eq!(used(client.store(), &open, VOLUMES_GIB), 4096);
    }

    #[tokio::test]
    async fn extend_is_checked_for_what_it_adds() {
        let (_dir, client) = client();
        let project = tenant(client.store(), "tight", &[(VOLUMES_GIB, 10)]);
        let vol = ready(&client, &project, "a", 4).await;
        let other = ready(&client, &project, "b", 4).await;

        let err = client.extend_volume(&vol.meta.id, 7).await.unwrap_err();
        refused(&err, VOLUMES_GIB, 8, 3, 10);
        let same = client.volumes().get(&vol.meta.id).unwrap();
        assert_eq!(same.spec.size_gib, 4);
        assert_eq!(same.status.phase, Phase::Ready);

        let grown = client.extend_volume(&vol.meta.id, 6).await.unwrap();
        assert_eq!(grown.spec.size_gib, 6);
        assert_eq!(used(client.store(), &project, VOLUMES_GIB), 10);

        client.reconcile_volume(&vol.meta.id).await.unwrap();
        client.request_delete_volume(&other.meta.id).unwrap();
        client.extend_volume(&vol.meta.id, 10).await.unwrap();
        assert_eq!(used(client.store(), &project, VOLUMES_GIB), 10);

        let open = tenant(client.store(), "open", &[]);
        let free = ready(&client, &open, "c", 1).await;
        client.extend_volume(&free.meta.id, 500).await.unwrap();
    }

    #[tokio::test]
    async fn snapshots_are_limited_and_a_delete_frees_one() {
        let (_dir, client) = client();
        let project = tenant(client.store(), "tight", &[(SNAPSHOTS, 2)]);
        let vol = ready(&client, &project, "a", 1).await;
        let first = snapshot(&client, &vol, "s1").await.unwrap();
        snapshot(&client, &vol, "s2").await.unwrap();
        assert_eq!(used(client.store(), &project, SNAPSHOTS), 2);
        let err = snapshot(&client, &vol, "s3").await.unwrap_err();
        refused(&err, SNAPSHOTS, 2, 1, 2);
        assert_eq!(client.snapshots().list(Some(&project)).unwrap().len(), 2);

        client.request_delete_snapshot(&first.meta.id).unwrap();
        assert_eq!(used(client.store(), &project, SNAPSHOTS), 1);
        snapshot(&client, &vol, "s3").await.unwrap();

        let open = tenant(client.store(), "open", &[]);
        let vol = ready(&client, &open, "a", 1).await;
        for i in 0..5 {
            snapshot(&client, &vol, &format!("s{i}")).await.unwrap();
        }
        assert_eq!(used(client.store(), &open, SNAPSHOTS), 5);
    }

    #[tokio::test]
    async fn backups_are_limited_by_count() {
        let (_dir, client) = client();
        let project = tenant(client.store(), "tight", &[(BACKUPS, 1)]);
        let vol = ready(&client, &project, "a", 2).await;
        let first = backup(&client, &vol, "b1").await.unwrap();
        assert_eq!(used(client.store(), &project, BACKUPS), 1);
        assert_eq!(used(client.store(), &project, BACKUPS_GIB), 2);
        let err = backup(&client, &vol, "b2").await.unwrap_err();
        refused(&err, BACKUPS, 1, 1, 1);
        assert_eq!(client.backups().list(Some(&project)).unwrap().len(), 1);

        client.request_delete_backup(&first.meta.id).unwrap();
        assert_eq!(used(client.store(), &project, BACKUPS), 0);
        assert_eq!(used(client.store(), &project, BACKUPS_GIB), 0);
        backup(&client, &vol, "b2").await.unwrap();
    }

    #[tokio::test]
    async fn backups_are_limited_by_gigabytes() {
        let (_dir, client) = client();
        let project = tenant(client.store(), "tight", &[(BACKUPS_GIB, 5)]);
        let small = ready(&client, &project, "a", 2).await;
        let large = ready(&client, &project, "b", 4).await;
        let first = backup(&client, &small, "b1").await.unwrap();
        let err = backup(&client, &large, "b2").await.unwrap_err();
        refused(&err, BACKUPS_GIB, 2, 4, 5);
        backup(&client, &small, "b3").await.unwrap();
        assert_eq!(used(client.store(), &project, BACKUPS_GIB), 4);

        client.request_delete_backup(&first.meta.id).unwrap();
        assert_eq!(used(client.store(), &project, BACKUPS_GIB), 2);
        assert!(backup(&client, &large, "b2").await.is_err());

        let open = tenant(client.store(), "open", &[]);
        let vol = ready(&client, &open, "a", 64).await;
        for i in 0..3 {
            backup(&client, &vol, &format!("b{i}")).await.unwrap();
        }
        assert_eq!(used(client.store(), &open, BACKUPS_GIB), 192);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_creates_stop_at_the_limit() {
        let (_dir, client) = client();
        let project = tenant(client.store(), "tight", &[(VOLUMES_GIB, 3)]);
        let tasks: Vec<_> = (0..12)
            .map(|i| {
                let (client, project) = (client.clone(), project.clone());
                tokio::spawn(
                    async move { create(&client, &project, &format!("v{i}"), 1).await.is_ok() },
                )
            })
            .collect();
        let mut made = 0;
        for t in tasks {
            if t.await.unwrap() {
                made += 1;
            }
        }
        assert_eq!(made, 3);
        assert_eq!(count(client.store(), &project).unwrap().volumes_gib, 3);
    }

    #[tokio::test]
    async fn the_hook_can_be_turned_off() {
        let dir = tempfile::tempdir().unwrap();
        let local = Client::local(dir.path()).unwrap();
        let client = Client::new(
            local.store().clone(),
            local.backend().clone(),
            Arc::new(AllowAll),
        );
        client.seed_defaults().unwrap();
        let project = tenant(client.store(), "tight", &[(VOLUMES, 0)]);
        create(&client, &project, "data", 1).await.unwrap();
    }
}
