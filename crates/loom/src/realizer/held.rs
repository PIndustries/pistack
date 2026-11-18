// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use std::collections::HashMap;
#[cfg(test)]
use std::sync::Arc;
use std::sync::Mutex;

use crate::realizer::holding::Holding;

/// What this process holds open, and how often each owner let go of it.
///
/// A balancer is recorded before its socket is bound, so losing the lead
/// closes it even when the pass that bound it failed afterwards. A pass
/// that was still binding when its owner let go closes what it opened
/// before it returns.
#[derive(Default)]
pub(crate) struct Held {
    pub(crate) ids: Mutex<HashMap<Id, Holding>>,
    /// Bumped by every release of an owner label.
    pub(crate) releases: Mutex<HashMap<&'static str, u64>>,
    /// Test hook: stop right after the bind until told to go on.
    #[cfg(test)]
    pub(crate) pause: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
}

impl Held {
    pub(crate) fn ids(&self) -> std::sync::MutexGuard<'_, HashMap<Id, Holding>> {
        self.ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// How many times `owner` has let go so far.
    pub(crate) fn term(&self, owner: &'static str) -> u64 {
        self.releases
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(owner)
            .copied()
            .unwrap_or(0)
    }

    /// Record `id` before its socket is bound.
    pub(crate) fn opening(&self, id: &Id, owner: &'static str) {
        self.ids().entry(id.clone()).or_insert(Holding {
            owner,
            ready: false,
        });
    }

    pub(crate) fn ready_here(&self, id: &Id) -> bool {
        self.ids().get(id).is_some_and(|h| h.ready)
    }

    pub(crate) fn mark_ready(&self, id: &Id, owner: &'static str) {
        self.ids()
            .insert(id.clone(), Holding { owner, ready: true });
    }

    /// Start a new term for `owner` and hand back every id it holds.
    pub(crate) fn release(&self, owner: &'static str) -> Vec<Id> {
        *self
            .releases
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(owner)
            .or_insert(0) += 1;
        let mut ids = self.ids();
        let gone: Vec<Id> = ids
            .iter()
            .filter(|(_, h)| h.owner == owner)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &gone {
            ids.remove(id);
        }
        gone
    }

    #[cfg(test)]
    pub(crate) async fn paused(&self) {
        let hook = self
            .pause
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some((reached, go)) = hook {
            reached.notify_one();
            go.notified().await;
        }
    }
}
