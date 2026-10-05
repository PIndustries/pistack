// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Event-driven reconciler: every write to a slab kind wakes it, it looks
//! at `status.phase` and asks the backend to make it true.

use pistack_types::{Event, EventAction};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::client::Client;
use crate::types::{KIND_BACKUP, KIND_SNAPSHOT, KIND_VOLUME};

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
                loop {
                    tokio::select! {
                        _ = active.cancelled() => {
                            tracing::debug!("slab reconciler stopping");
                            return;
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
        return;
    }
    let res = match ev.kind.as_str() {
        KIND_VOLUME => client.reconcile_volume(&ev.id).await.map(|_| ()),
        KIND_SNAPSHOT => client.reconcile_snapshot(&ev.id).await.map(|_| ()),
        KIND_BACKUP => client.reconcile_backup(&ev.id).await.map(|_| ()),
        _ => Ok(()),
    };
    if let Err(e) = res {
        tracing::warn!(kind = %ev.kind, id = %ev.id, error = %e, "slab: reconcile failed");
    }
}
