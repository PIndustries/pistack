// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use pistack_types::module::CancellationToken;
use tokio::sync::{broadcast, mpsc};

use super::coalense::COALENSE;
use super::notify_moved::notify_moved;
use super::notify_one::notify_one;

/// Send NOTIFYs for zones whose serial moved. Runs while this node is the
/// leader, beside the reconciler that moves them, so one node notifies for the
/// whole cloud. `active` is the leadership token: when it fires the loop stops
/// with the leadership.
pub(crate) async fn run_notifies(
    client: Client,
    mut events: broadcast::Receiver<pistack_types::Event>,
    mut forced: mpsc::UnboundedReceiver<String>,
    active: CancellationToken,
) {
    let mut window: Option<tokio::time::Instant> = None;
    loop {
        let due = async {
            match window {
                Some(t) => tokio::time::sleep_until(t).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = active.cancelled() => break,
            // A caller asked for this zone by name.
            Some(apex) = forced.recv() => {
                notify_one(&client, &apex).await;
            }
            ev = events.recv() => match ev {
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                    // The reconcile pass may not have run yet; give it the
                    // window, then read what the serials actually say.
                    window.get_or_insert_with(|| tokio::time::Instant::now() + COALENSE);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = due => {
                window = None;
                notify_moved(&client).await;
            }
        }
    }
}
