// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::group::Member;

/// After granting a vote, follow that candidate instead of the old leader.
pub fn follow_candidate(members: &[Member], candidate_id: &str) -> Option<String> {
    members
        .iter()
        .find(|member| member.id == candidate_id)
        .map(|member| member.api.clone())
}
