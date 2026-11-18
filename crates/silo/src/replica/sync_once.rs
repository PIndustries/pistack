// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::{Bucket, Object};
use pistack_types::ApiResult;
use std::time::Duration;

use super::prune_orphans::prune_orphans;
use super::sync_report::SyncReport;

/// Fetch the bytes of every object this server lacks.
///
/// With `prune`, also remove bytes whose record is gone and that are
/// older than `grace`. Only a follower prunes: the leader is where a put
/// writes the file before the record, and its deletes remove their own
/// bytes. A client with no peers does nothing.
pub async fn sync_once(client: &Client, prune: bool, grace: Duration) -> ApiResult<SyncReport> {
    let mut report = SyncReport::default();
    let Some(peers) = client.peers() else {
        return Ok(report);
    };
    if peers.urls().is_empty() {
        return Ok(report);
    }
    let buckets = client.store().collection::<Bucket>().list(None)?;
    let objects = client.store().collection::<Object>().list(None)?;
    for obj in &objects {
        if !buckets.iter().any(|b| b.meta.id == obj.spec.bucket) {
            continue;
        }
        match client.ensure_bytes(obj).await {
            Ok(true) => report.fetched += 1,
            Ok(false) => {}
            Err(e) => {
                report.missing += 1;
                tracing::warn!(
                    bucket = %obj.spec.bucket,
                    key = %obj.spec.key,
                    error = %e,
                    "silo: object bytes are on no server that answered"
                );
            }
        }
    }
    if prune {
        report.pruned = prune_orphans(client, &buckets, &objects, grace).await;
    }
    Ok(report)
}
