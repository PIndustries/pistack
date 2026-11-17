// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The reply to one store event: which resource pass it asks for, and the
//! revision check that recognises the event a status write just emitted.

use crate::client::Client;
use crate::types::kinds;
use pistack_types::{kind, Event, EventAction};

impl Client {
    /// React to one store event.
    pub async fn handle_event(&self, ev: &Event) {
        if ev.action == EventAction::Deleted {
            // Deletes talk to the provider inside `Client`, and a member
            // delete touches its pool there, so nothing is left to do.
            return;
        }
        // A status write is an event too. Acting on one would program the
        // provider and write the status again, without end, so an event
        // only counts when the resource changed since it was last looked
        // at. The periodic pass retries what failed.
        let seen = |observed: u64, revision: u64| observed == revision;
        match ev.kind.as_str() {
            kind::BALANCER => {
                if let Ok(Some(b)) = self.balancers().try_get(&ev.id) {
                    if seen(b.status.observed_revision, b.meta.revision) {
                        return;
                    }
                }
                self.reconcile_balancer(&ev.id).await
            }
            kinds::LISTENER => {
                if let Ok(Some(l)) = self.listeners().try_get(&ev.id) {
                    if seen(l.status.observed_revision, l.meta.revision) {
                        return;
                    }
                }
                self.reconcile_listener(&ev.id).await
            }
            kinds::POOL => {
                if let Ok(Some(p)) = self.pools().try_get(&ev.id) {
                    if seen(p.status.observed_revision, p.meta.revision) {
                        return;
                    }
                }
                self.reconcile_pool(&ev.id).await
            }
            kinds::MEMBER => self.reconcile_member(&ev.id).await,
            _ => {}
        }
    }
}
