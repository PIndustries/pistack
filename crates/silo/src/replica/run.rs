// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use super::prune_grace::PRUNE_GRACE;
use super::sync_once::sync_once;
use super::sync_report::SyncReport;

/// Pull every `interval` until `shutdown`. `is_leader` is asked before
/// each pass; a follower also prunes.
pub async fn run(
    client: Client,
    interval: Duration,
    is_leader: impl Fn() -> bool + Send + 'static,
    shutdown: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = tokio::time::sleep(interval) => {}
        }
        match sync_once(&client, !is_leader(), PRUNE_GRACE).await {
            Ok(report) if report != SyncReport::default() => tracing::info!(
                fetched = report.fetched,
                missing = report.missing,
                pruned = report.pruned,
                "silo: object bytes synced with the other servers"
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "silo: syncing object bytes failed"),
        }
        match crate::staging::sync(&client).await {
            Ok(report) if report != crate::staging::StagedReport::default() => tracing::info!(
                fetched = report.fetched,
                removed = report.removed,
                "silo: multipart staging synced with the other servers"
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "silo: syncing multipart staging failed"),
        }
    }
}
