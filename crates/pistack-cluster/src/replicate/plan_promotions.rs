// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::group::{Member, Role};

use super::voter_target::voter_target;

/// Learners to promote now, as the next membership, or `None`. Only learners
/// for which `caught_up` says yes are promoted, in member order, up to
/// [`voter_target`]. A step that would leave an even voter count short of
/// the target waits, so one voter and two learners become three voters
/// together rather than two voters that both must answer.
pub fn plan_promotions(
    members: &[Member],
    caught_up: impl Fn(&str) -> bool,
) -> Option<Vec<Member>> {
    let voters = members
        .iter()
        .filter(|member| member.role == Role::Voter)
        .count();
    let target = voter_target(members);
    if voters >= target {
        return None;
    }
    let ready: Vec<usize> = members
        .iter()
        .enumerate()
        .filter(|(_, member)| member.role == Role::Learner && caught_up(&member.id))
        .map(|(index, _)| index)
        .collect();
    let mut take = (target - voters).min(ready.len());
    if take > 0 && voters + take != target && (voters + take).is_multiple_of(2) {
        take -= 1;
    }
    if take == 0 {
        return None;
    }
    let mut next = members.to_vec();
    for index in ready.into_iter().take(take) {
        next[index].role = Role::Voter;
    }
    Some(next)
}
