// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Counting a reconcile failure.

use super::reconcile_failures::RECONCILE_FAILURES;

/// A reconcile pass, or one resource's step in it, ended in an error.
/// Call it where the error is logged.
pub fn reconcile_failed(module: &'static str) {
    let mut counts = RECONCILE_FAILURES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *counts.entry(module).or_insert(0) += 1;
}
