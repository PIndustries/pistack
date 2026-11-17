// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The task that runs the checker until the module shuts down.

use super::Checker;
use crate::client::Client;
use pistack_types::module::CancellationToken;
use std::time::Duration;

/// How often the checker looks for due probes.
pub(crate) const TICK: Duration = Duration::from_secs(1);

/// Run the checker until `shutdown` fires.
pub fn start(client: Client, shutdown: CancellationToken) {
    tokio::spawn(async move {
        let mut checker = Checker::new(client);
        let mut tick = tokio::time::interval(TICK);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::debug!("relay health checker stopping");
                    break;
                }
                _ = tick.tick() => {
                    checker.tick(false).await;
                }
            }
        }
    });
}
