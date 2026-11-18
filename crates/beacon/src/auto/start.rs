// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use pistack_types::module::CancellationToken;
use pistack_types::Event;
use tokio::sync::broadcast;

use super::resync::RESYNC;
use super::run_pass::run_pass;
use super::triggers::TRIGGERS;

/// Run `sync_all` now, then on every triggering event and every `RESYNC`,
/// until `shutdown` fires.
pub fn start(client: Client, mut rx: broadcast::Receiver<Event>, shutdown: CancellationToken) {
    tokio::spawn(async move {
        run_pass(&client);
        let mut tick = tokio::time::interval(RESYNC);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::debug!("beacon auto-records stopping");
                    break;
                }
                ev = rx.recv() => match ev {
                    Ok(ev) if TRIGGERS.contains(&ev.kind.as_str()) => run_pass(&client),
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => run_pass(&client),
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = tick.tick() => run_pass(&client),
            }
        }
    });
}
