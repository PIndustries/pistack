// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::multipart::scan_staged;
use chrono::{DateTime, Utc};
use std::time::Duration;

use super::bucket_dirs::bucket_dirs;
use super::sweep_report::SweepReport;

/// Remove every multipart staging dir under the client's root that was
/// initiated more than `ttl` before `now`.
///
/// Walks every bucket directory on disk, not only buckets in the store.
/// Unreadable entries are skipped and counted nowhere; a removal error is
/// logged and counted in [`SweepReport::failed`].
pub async fn sweep_expired_uploads(
    client: &Client,
    now: DateTime<Utc>,
    ttl: Duration,
) -> SweepReport {
    let mut report = SweepReport::default();
    let ttl = chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::MAX);
    let buckets = match bucket_dirs(client.root()).await {
        Ok(dirs) => dirs,
        Err(e) => {
            tracing::warn!(root = %client.root().display(), error = %e, "silo gc: cannot list data dir");
            return report;
        }
    };
    for bucket_dir in buckets {
        // An upload cannot outlive the TTL, so at twice that no server
        // still stages one this mark is there to keep away.
        crate::staging::sweep_done(&bucket_dir, ttl.to_std().unwrap_or_default() * 2).await;
        let staged = match scan_staged(&bucket_dir.join(".uploads")).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(bucket = %bucket_dir.display(), error = %e, "silo gc: cannot list uploads");
                continue;
            }
        };
        for upload in staged {
            report.scanned += 1;
            let Some(initiated) = upload.initiated else {
                continue;
            };
            if now.signed_duration_since(initiated) <= ttl {
                continue;
            }
            match tokio::fs::remove_dir_all(&upload.dir).await {
                Ok(()) => {
                    report.removed += 1;
                    tracing::info!(
                        upload_id = %upload.upload_id,
                        key = upload.meta.as_ref().map(|m| m.key.as_str()).unwrap_or(""),
                        initiated = %initiated,
                        "silo gc: removed expired multipart upload"
                    );
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    report.failed += 1;
                    tracing::warn!(
                        upload_id = %upload.upload_id,
                        error = %e,
                        "silo gc: cannot remove expired multipart upload"
                    );
                }
            }
        }
    }
    report
}
