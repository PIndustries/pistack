// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use chrono::Utc;
use pistack_types::CancellationToken;
use std::time::Duration;

use super::sweep_expired_uploads::sweep_expired_uploads;
use super::sweep_interval::sweep_interval;

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
