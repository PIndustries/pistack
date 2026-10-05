// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Sweep for abandoned multipart uploads.
//!
//! A client that creates a multipart upload and never completes or aborts
//! it leaves a staging dir under `{root}/{bucket}/.uploads/{uploadId}/`.
//! [`sweep_expired_uploads`] removes every staging dir whose initiation time
//! is older than the TTL (`[modules.silo] multipart_ttl_secs`, default 7
//! days). Age comes from `meta.json`'s `initiated` field, falling back to the
//! `meta.json` mtime, then the dir mtime, so uploads staged before that
//! field existed (or whose create crashed before `meta.json` was written)
//! are still collected.
//!
//! The sweep is a plain function over a [`Client`] and an explicit `now` so
//! tests drive it without a clock. [`run`] is the background loop the module
//! starts while this node is the cluster leader: it sweeps once immediately
//! (startup, or promotion) and then every [`sweep_interval`].
//!
//! Like S3 lifecycle expiry, the TTL counts from initiation, not from the
//! last part upload.

use crate::client::Client;
use crate::multipart::{io_err, scan_staged};
use chrono::{DateTime, Utc};
use pistack_types::{ApiResult, CancellationToken};
use std::path::Path;
use std::time::Duration;

/// Longest gap between sweeps. Shorter TTLs sweep every TTL.
pub const MAX_SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// What one sweep did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Staging dirs looked at.
    pub scanned: usize,
    /// Staging dirs removed because they were past the TTL.
    pub removed: usize,
    /// Expired dirs that could not be removed (logged; retried next sweep).
    pub failed: usize,
}

/// Gap between background sweeps for `ttl`: the TTL, capped at
/// [`MAX_SWEEP_INTERVAL`], never below one second.
pub fn sweep_interval(ttl: Duration) -> Duration {
    ttl.clamp(Duration::from_secs(1), MAX_SWEEP_INTERVAL)
}

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

/// Sweep now, then every [`sweep_interval`]`(ttl)`, until `active` is
/// cancelled. The module runs this under
/// [`pistack_types::Leadership::run_while_leader`].
pub async fn run(client: Client, ttl: Duration, active: CancellationToken) {
    let mut tick = tokio::time::interval(sweep_interval(ttl));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = active.cancelled() => {
                tracing::debug!("silo gc stopping");
                return;
            }
            _ = tick.tick() => {
                let report = sweep_expired_uploads(&client, Utc::now(), ttl).await;
                if report.removed > 0 || report.failed > 0 {
                    tracing::info!(
                        scanned = report.scanned,
                        removed = report.removed,
                        failed = report.failed,
                        "silo gc: multipart sweep"
                    );
                }
            }
        }
    }
}

/// Directories directly under `root` that can hold buckets (skips dot
/// entries such as `.tmp-*`). A missing root is empty.
async fn bucket_dirs(root: &Path) -> ApiResult<Vec<std::path::PathBuf>> {
    let mut entries = match tokio::fs::read_dir(root).await {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err(e)),
    };
    let mut out = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(io_err)? {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if entry.file_type().await.is_ok_and(|t| t.is_dir()) {
            out.push(entry.path());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_tracks_short_ttls_and_caps_long_ones() {
        assert_eq!(
            sweep_interval(Duration::from_secs(30)),
            Duration::from_secs(30)
        );
        assert_eq!(
            sweep_interval(Duration::from_secs(crate::MULTIPART_TTL_SECS)),
            MAX_SWEEP_INTERVAL
        );
        assert_eq!(sweep_interval(Duration::ZERO), Duration::from_secs(1));
    }
}
