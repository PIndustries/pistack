// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Image quotas: `images` and `images_bytes` per project. No project (a
//! cloud-wide image), no limit, or a project gate has no row for, means
//! unlimited. A disabled project refuses.
//!
//! Usage is counted from the image rows under gate's admission lock: one
//! per row, and the stored size of each that has data. Two images that
//! hold the same bytes share one blob on disk and are still counted
//! twice, since either can be deleted without the other.
//!
//! The size of an upload is not known until it has been stored, so
//! `images_bytes` is checked then: an upload that would go over the limit
//! is removed again and the image is left in `Error`. An upload to a
//! project that is already full is refused before a byte is read.

use gate::quota::{self, Demand, IMAGES, IMAGES_BYTES};
use pistack_store::Store;
use pistack_types::error::ErrorCode;
use pistack_types::{ApiResult, Id};

use crate::types::Image;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub images: u64,
    pub bytes: u64,
}

pub fn count(store: &Store, project: &Id) -> ApiResult<Usage> {
    let mut usage = Usage::default();
    for img in store.collection::<Image>().list(Some(project))? {
        usage.images += 1;
        usage.bytes += img.status.size_bytes.unwrap_or(0);
    }
    Ok(usage)
}

/// Refuse `more` images and `more_bytes` bytes on top of what the project
/// has. Admission lock held.
pub(crate) fn admit(
    store: &Store,
    project: Option<&Id>,
    more: u64,
    more_bytes: u64,
) -> ApiResult<()> {
    let Some(project) = project else {
        return Ok(());
    };
    let used = count(store, project)?;
    let demands = [
        Demand::new(IMAGES, used.images, more),
        Demand::new(IMAGES_BYTES, used.bytes, more_bytes),
    ];
    match quota::admit(store, project, &demands) {
        Err(e) if e.code == ErrorCode::NotFound => Ok(()),
        other => other,
    }
}

/// Count again and copy the result to the project. Admission lock held.
pub(crate) fn mirror(store: &Store, project: Option<&Id>) {
    let Some(project) = project else {
        return;
    };
    match count(store, project) {
        Ok(u) => quota::mirror_usage(
            store,
            project,
            &[(IMAGES, u.images), (IMAGES_BYTES, u.bytes)],
        ),
        Err(e) => tracing::warn!(%project, error = %e, "mold: usage was not recorded"),
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
    use crate::client::{Client, CreateImage};
    use crate::types::{ImageFormat, ImageSpec};
    use gate::{Project, ProjectSpec, ProjectStatus};
    use pistack_types::{kind, ApiError, Meta, Phase};

    fn client() -> (tempfile::TempDir, Client) {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::local(dir.path()).unwrap();
        (dir, client)
    }

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

    fn create(client: &Client, project: Option<&Id>, name: &str) -> ApiResult<Image> {
        let spec: ImageSpec = serde_json::from_value(serde_json::json!({
            "format": "raw",
            "arch": "x86_64",
        }))
        .unwrap();
        assert_eq!(spec.format, ImageFormat::Raw);
        client.create(CreateImage {
            name: name.into(),
            description: String::new(),
            labels: Default::default(),
            annotations: Default::default(),
            project: project.cloned(),
            spec,
        })
    }

    /// Upload `len` bytes whose content depends on `seed`.
    async fn upload(client: &Client, img: &Image, len: usize, seed: u8) -> ApiResult<Image> {
        let data: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_add(seed)).collect();
        client
            .upload(&img.meta.id, Box::pin(std::io::Cursor::new(data)))
            .await
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

    fn blobs(dir: &std::path::Path) -> usize {
        let mut n = 0;
        for e in std::fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                n += blobs(&e.path());
            } else {
                n += 1;
            }
        }
        n
    }

    #[tokio::test]
    async fn images_are_limited_and_a_delete_frees_one() {
        let (_dir, client) = client();
        let project = tenant(client.store(), "tight", &[(IMAGES, 2)]);
        let first = create(&client, Some(&project), "a").unwrap();
        create(&client, Some(&project), "b").unwrap();
        assert_eq!(used(client.store(), &project, IMAGES), 2);
        let err = create(&client, Some(&project), "c").unwrap_err();
        refused(&err, IMAGES, 2, 1, 2);
        assert_eq!(client.images().list(Some(&project)).unwrap().len(), 2);

        client.delete(&first.meta.id).await.unwrap();
        assert_eq!(used(client.store(), &project, IMAGES), 1);
        create(&client, Some(&project), "c").unwrap();
        assert_eq!(used(client.store(), &project, IMAGES), 2);
    }

    #[tokio::test]
    async fn no_image_limit_is_unlimited() {
        let (_dir, client) = client();
        let open = tenant(client.store(), "open", &[]);
        for i in 0..5 {
            let img = create(&client, Some(&open), &format!("i{i}")).unwrap();
            upload(&client, &img, 4096, i).await.unwrap();
        }
        assert_eq!(used(client.store(), &open, IMAGES), 5);
        assert_eq!(used(client.store(), &open, IMAGES_BYTES), 5 * 4096);

        // A cloud-wide image and a project gate does not know are not limited.
        create(&client, None, "base").unwrap();
        create(&client, Some(&Id::new(kind::PROJECT)), "stray").unwrap();
    }

    #[tokio::test]
    async fn image_bytes_are_limited_in_total() {
        let (dir, client) = client();
        let project = tenant(client.store(), "tight", &[(IMAGES_BYTES, 10_000)]);
        let a = create(&client, Some(&project), "a").unwrap();
        let b = create(&client, Some(&project), "b").unwrap();
        upload(&client, &a, 6000, 1).await.unwrap();
        assert_eq!(used(client.store(), &project, IMAGES_BYTES), 6000);

        // Over the limit: refused once the size is known, and nothing kept.
        let err = upload(&client, &b, 4001, 2).await.unwrap_err();
        refused(&err, IMAGES_BYTES, 6000, 4001, 10_000);
        let b = client.images().get(&b.meta.id).unwrap();
        assert_eq!(b.status.phase, Phase::Error);
        assert_eq!(b.status.size_bytes, None);
        assert_eq!(blobs(dir.path()), 1);
        assert_eq!(used(client.store(), &project, IMAGES_BYTES), 6000);

        // Exactly at the limit passes, on the same image.
        upload(&client, &b, 4000, 2).await.unwrap();
        assert_eq!(used(client.store(), &project, IMAGES_BYTES), 10_000);

        // Full: the next upload is refused before it is read.
        let c = create(&client, Some(&project), "c").unwrap();
        let err = upload(&client, &c, 1, 3).await.unwrap_err();
        refused(&err, IMAGES_BYTES, 10_000, 1, 10_000);
        let c = client.images().get(&c.meta.id).unwrap();
        assert_eq!(c.status.phase, Phase::Pending);

        // Deleting one frees its bytes.
        client.delete(&a.meta.id).await.unwrap();
        assert_eq!(used(client.store(), &project, IMAGES_BYTES), 4000);
        upload(&client, &c, 6000, 3).await.unwrap();
        assert_eq!(used(client.store(), &project, IMAGES_BYTES), 10_000);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_uploads_stop_at_the_limit() {
        let (_dir, client) = client();
        let project = tenant(client.store(), "tight", &[(IMAGES_BYTES, 3000)]);
        let mut tasks = Vec::new();
        for i in 0..8u8 {
            let img = create(&client, Some(&project), &format!("i{i}")).unwrap();
            let client = client.clone();
            tasks.push(tokio::spawn(async move {
                upload(&client, &img, 1000, i).await.is_ok()
            }));
        }
        let mut stored = 0;
        for t in tasks {
            if t.await.unwrap() {
                stored += 1;
            }
        }
        assert_eq!(stored, 3);
        assert_eq!(count(client.store(), &project).unwrap().bytes, 3000);
    }
}
