// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The per-process count of reconcile failures, by module.

use std::collections::BTreeMap;
use std::sync::Mutex;

pub(super) static RECONCILE_FAILURES: Mutex<BTreeMap<&'static str, u64>> =
    Mutex::new(BTreeMap::new());

/// Failures counted so far, by module. A module that has had none is not
/// listed.
pub fn reconcile_failures() -> BTreeMap<&'static str, u64> {
    RECONCILE_FAILURES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}
