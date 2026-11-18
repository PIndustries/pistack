// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// `at` with the low bits of a `prefix`-sized block cleared, so it starts on a
/// boundary a router would agree with.
pub(crate) fn align(at: u128, width: u16, prefix: u16) -> Option<u128> {
    let bits = width.saturating_sub(prefix);
    if bits >= 128 {
        return Some(0);
    }
    Some(at & !((1u128 << bits) - 1))
}
