// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Counts a module keeps for the operator. They live here because every
//! module depends on this crate and none of them on the metrics module.
//!
//! The counts are per process and start at zero with it.

use std::collections::BTreeMap;
use std::sync::Mutex;

static RECONCILE_FAILURES: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

/// A reconcile pass, or one resource's step in it, ended in an error.
/// Call it where the error is logged.
pub fn reconcile_failed(module: &'static str) {
    let mut counts = RECONCILE_FAILURES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *counts.entry(module).or_insert(0) += 1;
}

/// Failures counted so far, by module. A module that has had none is not
/// listed.
pub fn reconcile_failures() -> BTreeMap<&'static str, u64> {
    RECONCILE_FAILURES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_are_counted_per_module() {
        let before = reconcile_failures().get("tally-test").copied().unwrap_or(0);
        reconcile_failed("tally-test");
        reconcile_failed("tally-test");
        assert_eq!(reconcile_failures()["tally-test"], before + 2);
    }
}
