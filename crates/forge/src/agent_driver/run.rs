// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::registrar::Registrar;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;
impl Registrar {
    /// Consume agent events until `shutdown`.
    pub async fn run(self, shutdown: CancellationToken) {
        let mut rx = self.queue.subscribe_agent_events();
        // An agent may have said hello to the queue before this started
        // listening, as one does when it reaches a new leader first.
        self.resync();
        loop {
            let ev = tokio::select! {
                _ = shutdown.cancelled() => break,
                ev = rx.recv() => ev,
            };
            match ev {
                Ok(ev) => {
                    let node = ev.node_name().to_owned();
                    if let Err(e) = self.handle(ev) {
                        tracing::warn!(node, error = %e, "forge: agent event not applied");
                    }
                }
                Err(RecvError::Lagged(n)) => {
                    tracing::debug!(skipped = n, "forge: agent events lagged; resyncing");
                    self.resync();
                }
                Err(RecvError::Closed) => break,
            }
        }
        tracing::info!("forge: registrar stopped");
    }
}
