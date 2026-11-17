// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Pushing one listener's picture to the provider, publishing it through
//! its door if it has one, and recording what came back.

use crate::client::Client;
use crate::reconcile::stale;
use crate::types::Listener;
use pistack_types::Phase;

impl Client {
    /// Push a listener's current picture to the provider. With `force`
    /// false the call is skipped when the listener itself is up to date
    /// (used when only its pool's membership changed: the provider still
    /// needs the new member list, so callers pass `true` there too; `false`
    /// is for the retry pass).
    pub(super) async fn program_listener(&self, l: &Listener, force: bool) {
        if !force && !stale(l.status.observed_revision, l.meta.revision, l.status.phase) {
            return;
        }
        let view = match self.listener_view(l) {
            Ok(Some(v)) => v,
            Ok(None) => {
                // Balancer not realised yet; its own pass will come back.
                return;
            }
            Err(e) => {
                let _ = self.listeners().modify(&l.meta.id, |x| {
                    pistack_types::tally::reconcile_failed("relay");
                    x.status.phase = Phase::Error;
                    x.status.message = e.message.clone();
                    x.status.observed_revision = x.meta.revision + 1;
                    Ok(())
                });
                return;
            }
        };
        if self.leaves(&view.balancer) {
            return;
        }
        let result = match self.provider_for(&view.balancer) {
            Ok(p) => p.ensure_listener(&view).await,
            Err(e) => Err(e),
        };
        let result = match result {
            Ok(bound) => match view.listener.spec.door {
                Some(kind) => {
                    let upstream = format!("http://{bound}");
                    let listen = view.listener.spec.port.to_string();
                    match crate::door::apply(
                        &view.listener.meta.id.to_string(),
                        kind,
                        &listen,
                        &upstream,
                        crate::door::DoorSide {
                            store: self.store(),
                            queue: self.queue(),
                        },
                    )
                    .await
                    {
                        Ok(_) => Ok(bound),
                        Err(e) => Err(e),
                    }
                }
                None => Ok(bound),
            },
            Err(e) => Err(e),
        };
        let outcome = self.listeners().modify(&l.meta.id, |x| {
            match &result {
                Ok(r) => {
                    x.status.provider_ref = Some(r.clone());
                    x.status.phase = Phase::Ready;
                    x.status.message.clear();
                }
                Err(e) => {
                    pistack_types::tally::reconcile_failed("relay");
                    x.status.phase = Phase::Error;
                    x.status.message = e.message.clone();
                }
            }
            x.status.observed_revision = x.meta.revision + 1;
            Ok(())
        });
        if let Err(e) = outcome {
            tracing::warn!(listener = %l.meta.id, error = %e, "relay: status write failed");
        }
    }
}
