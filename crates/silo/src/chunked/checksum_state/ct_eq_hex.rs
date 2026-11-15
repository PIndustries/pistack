// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use subtle::ConstantTimeEq;

/// Constant-time hex compare (length mismatch → false without leaking content).
pub fn ct_eq_hex(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    bool::from(a.ct_eq(b))
}
