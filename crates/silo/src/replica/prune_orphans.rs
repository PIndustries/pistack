// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::{Bucket, Object};
use std::collections::BTreeSet;
use std::time::Duration;

use super::marker_dir::marker_dir;
use super::older_than::older_than;
use super::read_marker_file::read_marker_file;

/// Remove what a deleted record left on this server's disk.
pub(crate) async fn prune_orphans(
    client: &Client,
    buckets: &[Bucket],
    objects: &[Object],
    grace: Duration,
) -> usize {
    let mut pruned = 0;
    let Ok(mut dirs) = tokio::fs::read_dir(client.root()).await else {
        return 0;
    };
    while let Ok(Some(entry)) = dirs.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !entry.path().is_dir() {
            continue;
        }
        let Some(bucket) = buckets.iter().find(|b| b.meta.id.to_string() == name) else {
            // The bucket was deleted on the leader.
            if older_than(&entry.path(), grace).await
                && tokio::fs::remove_dir_all(entry.path()).await.is_ok()
            {
                tracing::info!(bucket = %name, "silo: removed the bytes of a deleted bucket");
                pruned += 1;
            }
            continue;
        };
        let live: Vec<&Object> = objects
            .iter()
            .filter(|o| o.spec.bucket == bucket.meta.id)
            .collect();
        let keys: BTreeSet<&str> = live
            .iter()
            .filter(|o| o.status.latest)
            .map(|o| o.spec.key.as_str())
            .collect();
        let shas: BTreeSet<&str> = live.iter().map(|o| o.status.sha256.as_str()).collect();

        if let Ok(mut markers) =
            tokio::fs::read_dir(marker_dir(client.root(), &bucket.meta.id)).await
        {
            while let Ok(Some(m)) = markers.next_entry().await {
                let Some(marker) = read_marker_file(&m.path()).await else {
                    continue;
                };
                if keys.contains(marker.key.as_str()) || !older_than(&m.path(), grace).await {
                    continue;
                }
                if let Ok(path) = client.key_path(&bucket.meta.id, &marker.key) {
                    let _ = tokio::fs::remove_file(&path).await;
                }
                let _ = tokio::fs::remove_file(m.path()).await;
                pruned += 1;
            }
        }
        if let Ok(mut blobs) = tokio::fs::read_dir(entry.path().join(".blobs")).await {
            while let Ok(Some(b)) = blobs.next_entry().await {
                let sha = b.file_name().to_string_lossy().into_owned();
                if shas.contains(sha.as_str()) || !older_than(&b.path(), grace).await {
                    continue;
                }
                if tokio::fs::remove_file(b.path()).await.is_ok() {
                    pruned += 1;
                }
            }
        }
    }
    pruned
}
