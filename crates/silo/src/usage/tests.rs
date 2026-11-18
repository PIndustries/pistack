// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use gate::quota::BUCKETS;
use pistack_store::Store;
use pistack_types::error::ErrorCode;
use pistack_types::{ApiResult, Id};

use crate::types::Bucket;

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
            std::thread::spawn(move || bucket(&client, Some(&project), &format!("b{i}")).is_ok())
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
