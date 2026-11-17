// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::*;

#[test]
fn failures_are_counted_per_module() {
    let before = reconcile_failures().get("tally-test").copied().unwrap_or(0);
    reconcile_failed("tally-test");
    reconcile_failed("tally-test");
    assert_eq!(reconcile_failures()["tally-test"], before + 2);
}
