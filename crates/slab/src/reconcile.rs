// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Event-driven reconciler: every write to a slab kind wakes it, it looks
//! at `status.phase` and asks the backend to make it true. A pass also
//! drops attaches whose machine record is gone, and plugs shared volumes
//! back into machines that were evacuated to another host.

use pistack_types::{kind, Event, EventAction};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::client::Client;
use crate::types::{KIND_BACKUP, KIND_SNAPSHOT, KIND_VOLUME};

/// How often to drop attaches whose machine no longer exists, even when
/// no machine-delete event arrived. Evacuated machines are checked on the
/// same tick.
const STALE_ATTACH_EVERY: Duration = Duration::from_secs(30);

pub async fn run(
    client: Client,
    events: broadcast::Sender<Event>,
    shutdown: CancellationToken,
    leadership: pistack_types::Leadership,
) {
    leadership
        .run_while_leader(shutdown, move |active| {
            let client = client.clone();
            let events = events.clone();
            async move {
                if let Err(e) = client.seed_defaults() {
                    tracing::warn!(error = %e, "slab: seed defaults");
                }
                if let Err(e) = client.reconcile_all().await {
                    tracing::warn!(error = %e, "slab: initial resync failed");
                }
                let mut events = events.subscribe();
                let mut tick = tokio::time::interval(STALE_ATTACH_EVERY);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                // `interval` completes once immediately. Startup already reconciled.
                tick.tick().await;
                loop {
                    tokio::select! {
                        _ = active.cancelled() => {
                            tracing::debug!("slab reconciler stopping");
                            return;
                        }
                        _ = tick.tick() => {
                            if let Err(e) = client.reconcile_stale_attaches().await {
                                tracing::warn!(error = %e, "slab: stale attach reconcile failed");
                            }
                            if let Err(e) = client.reattach_evacuated(None).await {
                                tracing::warn!(error = %e, "slab: evacuated attach reconcile failed");
                            }
                        }
                        ev = events.recv() => match ev {
                            Ok(ev) => handle(&client, ev).await,
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!(missed = n, "slab: event stream lagged; resyncing");
                                if let Err(e) = client.reconcile_all().await {
                                    tracing::warn!(error = %e, "slab: resync failed");
                                }
                            }
                            Err(broadcast::error::RecvError::Closed) => return,
                        }
                    }
                }
            }
        })
        .await;
}

async fn handle(client: &Client, ev: Event) {
    if ev.action == EventAction::Deleted {
        if ev.kind == kind::MACHINE {
            if let Err(e) = client.reconcile_stale_attaches().await {
                tracing::warn!(
                    machine = %ev.id,
                    error = %e,
                    "slab: stale attach reconcile failed"
                );
            }
        }
        return;
    }
    let res = match ev.kind.as_str() {
        // A machine that became ready on a new host after an evacuation.
        kind::MACHINE => client.reattach_evacuated(Some(&ev.id)).await,
        KIND_VOLUME => client.reconcile_volume(&ev.id).await.map(|_| ()),
        KIND_SNAPSHOT => client.reconcile_snapshot(&ev.id).await.map(|_| ()),
        KIND_BACKUP => client.reconcile_backup(&ev.id).await.map(|_| ()),
        _ => Ok(()),
    };
    if let Err(e) = res {
        tracing::warn!(kind = %ev.kind, id = %ev.id, error = %e, "slab: reconcile failed");
    }
}
