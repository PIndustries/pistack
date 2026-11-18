// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::group::{Member, Role};

/// How many voters a membership should have: three once three servers are
/// members, otherwise the voters it already has (at least one). The second
/// server stays a learner, as [`crate::admit`] always did.
pub fn voter_target(members: &[Member]) -> usize {
    let voters = members
        .iter()
        .filter(|member| member.role == Role::Voter)
        .count();
    if members.len() >= 3 {
        3.max(voters)
    } else {
        voters.max(1)
    }
}
