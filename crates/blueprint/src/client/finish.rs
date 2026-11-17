// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{StackPhase, StackStatus};
use pistack_types::ApiError;

use super::applied::Applied;

/// Store the outcome of one realize on `status`.
pub(crate) fn finish(
    status: &mut StackStatus,
    managed: &[String],
    out: &Applied,
    err: Option<&ApiError>,
) {
    let mut all = managed.to_vec();
    for id in &out.created {
        if !all.contains(id) {
            all.push(id.clone());
        }
    }
    all.retain(|id| !out.deleted.iter().any(|d| d == id));
    status.managed = all;
    status.created = out.created.clone();
    status.updated = out.updated.clone();
    status.left = out.left.clone();
    match err {
        None => {
            status.phase = StackPhase::Ready;
            status.message = out.message();
        }
        Some(e) => {
            status.phase = StackPhase::Failed;
            let mut parts = vec![format!("apply failed: {}", e.message)];
            parts.extend(out.progress());
            status.message = parts.join("; ");
        }
    }
}
