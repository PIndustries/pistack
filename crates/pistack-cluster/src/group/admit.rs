// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::member::Member;
use super::role::Role;

/// Assign the joining node's role and return the new membership.
/// The caller's `role` on `newbie` is ignored.
pub fn admit(members: &[Member], mut newbie: Member) -> Vec<Member> {
    let voters = members
        .iter()
        .filter(|member| member.role == Role::Voter)
        .count();
    let learners = members
        .iter()
        .filter(|member| member.role == Role::Learner)
        .count();
    let promote = voters == 1 && learners >= 1;
    newbie.role = if voters >= 3 || (voters == 1 && learners == 0) {
        Role::Learner
    } else {
        Role::Voter
    };
    let mut next = members.to_vec();
    next.push(newbie);
    if promote {
        for member in &mut next {
            member.role = Role::Voter;
        }
    }
    next
}
