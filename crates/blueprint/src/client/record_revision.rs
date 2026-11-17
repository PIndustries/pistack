// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{StackRevision, StackSpec, StackStatus, REVISION_HISTORY_LIMIT};

/// Record `spec` as the newest revision unless it already is.
pub(crate) fn record_revision(
    status: &mut StackStatus,
    spec: &StackSpec,
    restored_from: Option<u64>,
) {
    if let Some(last) = status.history.last() {
        if &last.spec == spec {
            status.revision = last.revision;
            return;
        }
    }
    let newest = status.history.last().map_or(0, |r| r.revision);
    let revision = newest.max(status.revision) + 1;
    status.history.push(StackRevision {
        revision,
        spec: spec.clone(),
        applied_at: chrono::Utc::now(),
        restored_from,
    });
    if status.history.len() > REVISION_HISTORY_LIMIT {
        let extra = status.history.len() - REVISION_HISTORY_LIMIT;
        status.history.drain(..extra);
    }
    status.revision = revision;
}
