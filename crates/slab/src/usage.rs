// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Volume quota. No project, or no `volumes` limit, means unlimited. A
//! missing project row is the same. A disabled project rejects a create.
//! Usage is counted when the row is stored and released when the delete is
//! accepted, once. A bookkeeping failure is logged and does not remove the row.

use crate::types::Volume;
use gate::{check_quota, record_usage, Project};
use pistack_store::Store;
use pistack_types::error::ErrorCode;
use pistack_types::{ApiResult, Id, Phase};

const VOLUMES: &str = "volumes";
/// Set the first time a delete is accepted, so a later delete does not release again.
const RELEASED: &str = "pistack.io/volume-quota-released";

pub fn admit_create(store: &Store, project: Option<&Id>) -> ApiResult<()> {
    let Some(project) = project else {
        return Ok(());
    };
    match check_quota(store, project, VOLUMES, 1) {
        Err(e) if e.code == ErrorCode::NotFound => Ok(()),
        other => other,
    }
}

pub fn note_create(store: &Store, project: Option<&Id>) {
    adjust(store, project, 1);
}

pub fn note_delete(store: &Store, project: Option<&Id>) {
    adjust(store, project, -1);
}

/// Mark `volume` deleting. True only the first time its quota slot should drop.
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

fn adjust(store: &Store, project: Option<&Id>, delta: i64) {
    let Some(project) = project else {
        return;
    };
    match store.collection::<Project>().try_get(project) {
        Ok(None) => return,
        Ok(Some(_)) => {}
        Err(e) => {
            tracing::warn!(%project, delta, error = %e, "slab: volume usage was not recorded");
            return;
        }
    }
    if let Err(e) = record_usage(store, project, VOLUMES, delta) {
        tracing::warn!(%project, delta, error = %e, "slab: volume usage was not recorded");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Create;
    use crate::types::{VolumeSource, VolumeSpec};
    use crate::Client;
    use gate::{ProjectSpec, ProjectStatus};
    use pistack_types::{kind, Meta};

    fn tenant(store: &Store, name: &str, limit: Option<u64>) -> Id {
        let mut spec = ProjectSpec::default();
        if let Some(limit) = limit {
            spec.quotas.insert(VOLUMES.into(), limit);
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

    fn used(store: &Store, project: &Id) -> u64 {
        store
            .collection::<Project>()
            .get(project)
            .unwrap()
            .status
            .usage
            .get(VOLUMES)
            .copied()
            .unwrap_or(0)
    }

    fn client() -> (tempfile::TempDir, Client) {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::local(dir.path()).unwrap();
        client.seed_defaults().unwrap();
        (dir, client)
    }

    async fn create(client: &Client, project: &Id, name: &str) -> ApiResult<Volume> {
        client
            .create_volume(Create {
                name: name.into(),
                description: String::new(),
                labels: Default::default(),
                annotations: Default::default(),
                project: Some(project.clone()),
                spec: VolumeSpec {
                    size_gib: 1,
                    volume_type: "standard".into(),
                    source: VolumeSource::None,
                    bootable: false,
                    encrypted: false,
                },
            })
            .await
    }

    #[tokio::test]
    async fn a_zero_volume_quota_refuses_and_leaves_no_row() {
        let (_dir, client) = client();
        let project = tenant(client.store(), "tight", Some(0));
        let err = create(&client, &project, "data").await.unwrap_err();
        assert_eq!(err.code, ErrorCode::QuotaExceeded);
        assert!(client.volumes().list(Some(&project)).unwrap().is_empty());
        assert_eq!(used(client.store(), &project), 0);
    }

    #[tokio::test]
    async fn no_volume_quota_creates_and_delete_releases_once() {
        let (_dir, client) = client();
        let project = tenant(client.store(), "open", None);
        let vol = create(&client, &project, "data").await.unwrap();
        assert_eq!(client.volumes().list(Some(&project)).unwrap().len(), 1);
        assert_eq!(used(client.store(), &project), 1);

        client.request_delete_volume(&vol.meta.id).unwrap();
        assert_eq!(used(client.store(), &project), 0);
        client.request_delete_volume(&vol.meta.id).unwrap();
        assert_eq!(used(client.store(), &project), 0);
        assert_eq!(client.volumes().list(Some(&project)).unwrap().len(), 1);

        let again = create(&client, &project, "other").await.unwrap();
        assert_eq!(used(client.store(), &project), 1);
        client.request_delete_volume(&again.meta.id).unwrap();
        assert_eq!(used(client.store(), &project), 0);
    }
}
