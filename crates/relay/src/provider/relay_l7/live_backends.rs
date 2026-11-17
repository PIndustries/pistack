// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The members one pool offers: its primaries, or all of them when no
//! primary is left.

use crate::picker::Backend;
use crate::types::Member;

/// The members to choose between. A backup member is only used when no
/// primary is left; the rest of the health and weight rules are the
/// picker's.
pub(super) fn live_backends(members: &[Member]) -> Vec<Backend> {
    let primary: Vec<Backend> = members
        .iter()
        .filter(|m| !m.spec.backup)
        .map(Backend::of)
        .collect();
    if !primary.is_empty() {
        return primary;
    }
    members.iter().map(Backend::of).collect()
}
