// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use parking_lot::Mutex;
use pistack_types::module::CancellationToken;
use std::collections::HashSet;
use std::sync::Arc;

use super::dns_port::DNS_PORT;
use super::notify_message::notify_message;
use super::parse_addr::parse_addr;
use super::send_notify::send_notify;

/// Send one NOTIFY for `apex` at `serial` to each address in `peers`, with the
/// same ack and backoff the leader loop uses, and report whether every peer
/// answered. This is what a primary does on a serial change; the leader loop
/// reaches it through [`peers_for`], a caller that already knows its peers
/// reaches it directly.
pub async fn notify_peers(peers: &[String], apex: &str, serial: u32) -> anyhow::Result<()> {
    let bytes = notify_message(apex, serial)?;
    let acked = Arc::new(Mutex::new(HashSet::new()));
    let shutdown = CancellationToken::new();
    let mut tasks = Vec::new();
    for raw in peers {
        let addr = parse_addr(raw, DNS_PORT)?;
        let bytes = bytes.clone();
        let acked = acked.clone();
        let shutdown = shutdown.clone();
        let apex = apex.to_owned();
        tasks.push(tokio::spawn(async move {
            send_notify(&bytes, addr, &acked, &apex, &shutdown).await
        }));
    }
    let mut out = Ok(());
    for task in tasks {
        if !task
            .await
            .map_err(|e| anyhow::anyhow!("notify task: {e}"))?
        {
            out = Err(anyhow::anyhow!("a peer did not acknowledge the NOTIFY"));
        }
    }
    out
}
