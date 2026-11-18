// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::{Collection, StoreError};
use pistack_types::{ApiResult, Id, Phase};

use super::observed::Observed;

/// Write a status outcome. `extra` runs before the phase is set and may
/// fill kind-specific status fields.
///
/// `acted` is the revision this pass read. A spec write during the backend
/// call must stay stale: the event loop handles one event at a time, so
/// that write's event is still queued, and claiming it here would skip the
/// backend work for the new spec.
pub(crate) fn finish<K: Observed>(
    col: &Collection<K>,
    id: &Id,
    acted: u64,
    outcome: ApiResult<()>,
    mut extra: impl FnMut(&mut K),
) -> Phase {
    if matches!(&outcome, Err(e) if e.message == "not the leader") {
        tracing::debug!(kind = K::KIND, %id, "reconcile stopped; not the leader");
        return Phase::Updating;
    }
    let (phase, msg) = match &outcome {
        Ok(()) => (Phase::Ready, String::new()),
        Err(e) => (Phase::Error, e.to_string()),
    };
    // A retry that fails the same way has nothing new to say. Writing it
    // again would add a log entry per failed resource on every pass.
    if phase == Phase::Error {
        if let Ok(cur) = col.get(id) {
            if cur.meta().revision == acted
                && !cur.unseen()
                && cur.phase() == Phase::Error
                && cur.message() == msg
            {
                tracing::debug!(kind = K::KIND, %id, error = %msg, "reconcile still failing");
                return phase;
            }
        }
    }
    let res = col.modify(id, |cur| {
        if cur.meta().revision != acted {
            return Err(StoreError::Conflict("reconcile superseded".into()));
        }
        extra(cur);
        let next = cur.meta().revision + 1;
        cur.set_result(phase, msg.clone(), next);
        Ok(())
    });
    match res {
        Ok(_) => {
            if phase == Phase::Error {
                pistack_types::tally::reconcile_failed("fabric");
                tracing::warn!(kind = K::KIND, %id, error = %msg, "reconcile failed");
            } else {
                tracing::debug!(kind = K::KIND, %id, "reconciled");
            }
            phase
        }
        // Deleted while we worked; the Deleted event cleans up.
        Err(StoreError::NotFound(_)) => phase,
        Err(StoreError::NotLeader) => {
            tracing::debug!(kind = K::KIND, %id, "status write skipped; not the leader");
            Phase::Updating
        }
        Err(StoreError::Conflict(reason)) if reason == "reconcile superseded" => {
            tracing::debug!(kind = K::KIND, %id, "spec changed during reconcile");
            Phase::Updating
        }
        Err(e) => {
            tracing::error!(kind = K::KIND, %id, error = %e, "status write failed");
            phase
        }
    }
}
