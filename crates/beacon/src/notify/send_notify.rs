// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use parking_lot::Mutex;
use pistack_types::module::CancellationToken;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;

use super::ack_timeout::ACK_TIMEOUT;
use super::attempts::ATTEMPTS;
use super::backoff::BACKOFF;
use super::exchange::exchange;

/// Send one NOTIFY to `to` and wait for the answer, with backoff. True when
/// the peer answered.
pub(crate) async fn send_notify(
    bytes: &[u8],
    to: SocketAddr,
    acked: &Arc<Mutex<HashSet<(String, SocketAddr)>>>,
    apex: &str,
    shutdown: &CancellationToken,
) -> bool {
    let mut wait = BACKOFF;
    for attempt in 1..=ATTEMPTS {
        if shutdown.is_cancelled() {
            return false;
        }
        // The same address can stand for two zones; if a task for it already
        // heard back, there is nothing left to ask.
        if acked.lock().contains(&(apex.to_owned(), to)) {
            return true;
        }
        match tokio::time::timeout(ACK_TIMEOUT, exchange(bytes, to)).await {
            Ok(Ok(_)) => return true,
            Ok(Err(e)) => tracing::debug!(to = %to, error = %e, "beacon: notify"),
            Err(_) => tracing::debug!(to = %to, attempt, "beacon: notify not acknowledged"),
        }
        if attempt == ATTEMPTS {
            break;
        }
        tokio::select! {
            _ = shutdown.cancelled() => return false,
            _ = tokio::time::sleep(wait) => {}
        }
        wait *= 2;
    }
    false
}
