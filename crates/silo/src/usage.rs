// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Bucket quota: `buckets` per project. No project, no limit, or a project
//! gate has no row for, means unlimited. A disabled project refuses.
//!
//! The count is the project's bucket rows, taken under gate's admission
//! lock. There is no limit on object bytes: silo keeps no running total
//! per bucket or project, so the only way to know the usage is to read
//! every object record, and a put is too frequent for that.

use gate::quota::{self, Demand, BUCKETS};
use pistack_store::Store;
use pistack_types::error::ErrorCode;
use pistack_types::{ApiResult, Id};

use crate::types::Bucket;

pub fn count(store: &Store, project: &Id) -> ApiResult<u64> {
    Ok(store.collection::<Bucket>().count(Some(project))? as u64)
}

/// Run `create`, which stores one more bucket in `project`, unless the
/// project is at its limit.
pub(crate) fn admitted<T>(
    store: &Store,
    project: Option<&Id>,
    create: impl FnOnce() -> ApiResult<T>,
) -> ApiResult<T> {
    let Some(project) = project else {
        return create();
    };
    let _admission = quota::admission();
    let used = count(store, project)?;
    match quota::admit(store, project, &[Demand::new(BUCKETS, used, 1)]) {
        Err(e) if e.code == ErrorCode::NotFound => {}
        other => other?,
    }
    let out = create()?;
    quota::mirror_usage(store, project, &[(BUCKETS, used + 1)]);
    Ok(out)
}

/// Record the count again after a bucket was deleted.
pub(crate) fn released(store: &Store, project: Option<&Id>) {
    let Some(project) = project else {
        return;
    };
    let _admission = quota::admission();
    match count(store, project) {
        Ok(n) => quota::mirror_usage(store, project, &[(BUCKETS, n)]),
        Err(e) => tracing::warn!(%project, error = %e, "silo: usage was not recorded"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::BucketSpec;
    use crate::Client;
    use gate::{Project, ProjectSpec, ProjectStatus};
    use pistack_types::{kind, Meta, Spec};

    fn client() -> (tempfile::TempDir, Client) {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::new(Store::in_memory().unwrap(), dir.path());
        (dir, client)
    }

    fn tenant(store: &Store, name: &str, limit: Option<u64>) -> Id {
        let mut spec = ProjectSpec::default();
        if let Some(limit) = limit {
            spec.quotas.insert(BUCKETS.into(), limit);
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
            .get(BUCKETS)
            .copied()
            .unwrap_or(0)
    }

    fn bucket(client: &Client, project: Option<&Id>, name: &str) -> ApiResult<Bucket> {
        client.create_bucket(
            Meta::new(BucketSpec::KIND, name, project.cloned()),
            BucketSpec::default(),
        )
    }

    #[tokio::test]
    async fn buckets_are_limited_and_a_delete_frees_one() {
        let (_dir, client) = client();
        let project = tenant(client.store(), "tight", Some(2));
        let first = bucket(&client, Some(&project), "a").unwrap();
        bucket(&client, Some(&project), "b").unwrap();
        assert_eq!(used(client.store(), &project), 2);

        let err = bucket(&client, Some(&project), "c").unwrap_err();
        assert_eq!(err.code, ErrorCode::QuotaExceeded);
        assert_eq!(
            err.message,
            "quota exceeded for buckets in project 'tight': 2 used + 1 requested > 2"
        );
        assert_eq!(err.details["resource"], BUCKETS);
        assert_eq!(err.details["limit"], 2);
        assert_eq!(err.details["used"], 2);
        assert_eq!(err.details["requested"], 1);
        assert_eq!(client.list_buckets(Some(&project)).unwrap().len(), 2);

        client.delete_bucket(&first.meta.id).await.unwrap();
        assert_eq!(used(client.store(), &project), 1);
        bucket(&client, Some(&project), "c").unwrap();
        assert_eq!(used(client.store(), &project), 2);
    }

    #[tokio::test]
    async fn no_bucket_limit_is_unlimited() {
        let (_dir, client) = client();
        let open = tenant(client.store(), "open", None);
        for i in 0..5 {
            bucket(&client, Some(&open), &format!("b{i}")).unwrap();
        }
        assert_eq!(used(client.store(), &open), 5);

        // No project, and a project gate does not know, are not limited.
        bucket(&client, None, "shared").unwrap();
        bucket(&client, Some(&Id::new(kind::PROJECT)), "stray").unwrap();
    }

    #[test]
    fn concurrent_creates_stop_at_the_limit() {
        let (_dir, client) = client();
        let project = tenant(client.store(), "tight", Some(3));
        let threads: Vec<_> = (0..12)
            .map(|i| {
                let (client, project) = (client.clone(), project.clone());
                std::thread::spawn(move || {
                    bucket(&client, Some(&project), &format!("b{i}")).is_ok()
                })
            })
            .collect();
        let made = threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|ok| *ok)
            .count();
        assert_eq!(made, 3);
        assert_eq!(count(client.store(), &project).unwrap(), 3);
    }
}
