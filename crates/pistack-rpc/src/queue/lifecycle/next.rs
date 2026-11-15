// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use tokio::time::Instant;

use crate::proto::Command;
use crate::queue::command_queue::CommandQueue;

impl CommandQueue {
    /// Agent long-poll: up to `max` commands for `node`, waiting at most
    /// `wait` for the first one. Returns an empty vector on timeout.
    /// Commands that expired while queued are skipped.
    pub async fn next(&self, node: &str, wait: Duration, max: usize) -> Vec<Command> {
        self.reap();
        let max = max.max(1);
        let rx = {
            let mut st = self.0.state.lock();
            self.entry(&mut st, node).rx.clone()
        };
        // One poller per node at a time; a second concurrent poll waits here
        // and gets whatever the first leaves behind.
        let mut rx = rx.lock().await;
        let deadline = Instant::now() + wait;
        let mut out = Vec::new();
        loop {
            while out.len() < max {
                match rx.try_recv() {
                    Ok(cmd) => {
                        if self.mark_delivered(cmd.id) {
                            out.push(cmd);
                        }
                    }
                    Err(_) => break,
                }
            }
            if !out.is_empty() {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            match tokio::time::timeout(deadline - now, rx.recv()).await {
                Ok(Some(cmd)) => {
                    if self.mark_delivered(cmd.id) {
                        out.push(cmd);
                    }
                }
                // Channel closed (node removed) or timed out.
                Ok(None) | Err(_) => break,
            }
        }
        if !out.is_empty() {
            tracing::debug!(node, count = out.len(), "commands delivered");
        }
        out
    }
}
