// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::server::SecondaryCache;
use pistack_types::module::CancellationToken;
use std::net::IpAddr;
use tokio::sync::mpsc;

/// Pull a zone the moment a NOTIFY asked for it. Runs on every node that
/// serves DNS: a NOTIFY can arrive at any of them, and a node that is not the
/// leader still has to act on one it accepted.
pub(crate) async fn run_pulls(
    client: Client,
    cache: SecondaryCache,
    mut pulled: mpsc::UnboundedReceiver<(String, Option<IpAddr>)>,
    shutdown: CancellationToken,
) {
    while let Some((apex, from)) = pulled.recv().await {
        if shutdown.is_cancelled() {
            break;
        }
        crate::transfer_one(&client, &cache, &apex, from).await;
    }
}
