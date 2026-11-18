// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;
use pistack_rpc::proto::CommandKind;
use std::collections::BTreeMap;

use super::overlay_due;
use super::overlay_failed;
use super::reconciler::Reconciler;
use super::OverlaySent;

impl Reconciler {
    /// Push the overlay plan to each host that has already said hello, when
    /// the plan changed, the agent said hello again (it restarted), or the
    /// last apply failed more than [`OVERLAY_RETRY`] ago. The send is not
    /// awaited here: a background task records a failed answer.
    pub(super) async fn sync_overlay(&self) {
        if !crate::overlay::enabled() {
            return;
        }
        let Some(queue) = &self.queue else {
            return;
        };
        let plans = match crate::overlay::all_plans(&self.store) {
            Ok(plans) => plans,
            Err(e) => {
                tracing::warn!(error = %e, "forge: overlay plan was not built");
                return;
            }
        };
        let known: BTreeMap<String, Option<chrono::DateTime<Utc>>> = queue
            .nodes()
            .into_iter()
            .map(|n| (n.name, n.last_hello))
            .collect();
        for (node, plan) in plans {
            let Some(&hello) = known.get(&node) else {
                continue;
            };
            let body = match serde_json::to_string(&plan) {
                Ok(body) => body,
                Err(e) => {
                    tracing::warn!(node, error = %e, "forge: overlay plan was not encoded");
                    continue;
                }
            };
            if !overlay_due(self.overlay_sent.lock().get(&node), &body, hello) {
                continue;
            }
            let payload = serde_json::to_value(&plan).unwrap_or(serde_json::Value::Null);
            match queue.enqueue(&node, CommandKind::SyncOverlay, payload) {
                Ok(ticket) => {
                    self.overlay_sent.lock().insert(
                        node.clone(),
                        OverlaySent {
                            body: body.clone(),
                            hello,
                            failed: None,
                        },
                    );
                    let sent = self.overlay_sent.clone();
                    tokio::spawn(async move {
                        let why = match ticket.wait().await {
                            Ok(r) if r.ok => return,
                            Ok(r) => r.message,
                            Err(e) => e.to_string(),
                        };
                        tracing::warn!(node, error = %why, "forge: overlay apply failed; will retry");
                        overlay_failed(&sent, &node, &body);
                    });
                }
                Err(e) => {
                    tracing::warn!(node, error = %e, "forge: overlay plan was not queued");
                }
            }
        }
    }
}
