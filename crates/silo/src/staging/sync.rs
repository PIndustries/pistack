// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::Bucket;
use pistack_types::{ApiResult, Id, Spec};

use super::staged_report::StagedReport;
use super::want::Want;

/// Take what the other servers stage and this one lacks, and remove
/// uploads they saw finish. A client with no peers does nothing.
pub async fn sync(client: &Client) -> ApiResult<StagedReport> {
    let mut report = StagedReport::default();
    let Some(peers) = client.peers() else {
        return Ok(report);
    };
    for base in peers.urls() {
        let list = match crate::staging_fetch::list_staged(client, &base).await {
            Ok(list) => list,
            Err(e) => {
                tracing::debug!(peer = %base, error = %e, "silo: peer did not list its staging");
                continue;
            }
        };
        for done in &list.done {
            let Some(bucket) = Id::parse(crate::types::BucketSpec::KIND, &done.bucket) else {
                continue;
            };
            if client.store().collection::<Bucket>().get(&bucket).is_err()
                || client.upload_is_done(&bucket, &done.upload_id).await
            {
                continue;
            }
            let Ok(dir) = client.upload_dir(&bucket, &done.upload_id) else {
                continue;
            };
            // The mark first: a pass that stops here never takes it back.
            if client
                .mark_upload_done(&bucket, &done.upload_id)
                .await
                .is_err()
            {
                continue;
            }
            if tokio::fs::remove_dir_all(&dir).await.is_ok() {
                report.removed += 1;
            }
        }
        for upload in &list.uploads {
            match client.take(&base, upload, Want::All).await {
                Ok(n) => report.fetched += n,
                Err(e) => tracing::debug!(
                    peer = %base,
                    upload_id = %upload.upload_id,
                    error = %e,
                    "silo: could not copy a staged upload"
                ),
            }
        }
    }
    Ok(report)
}
