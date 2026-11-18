// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::group::{Member, Role};

use super::removal_refused::RemovalRefused;

/// The membership without `id`, or why it must stay.
///
/// `leader_id` is the server doing the removal and counts as reachable;
/// `reachable` says which other servers it heard from lately. Taking out a
/// voter is refused when fewer than a majority of the voters left are
/// reachable, because the cluster would stop committing the moment the
/// change is made. `force` skips that check and nothing else. A learner has
/// no vote, so removing one is never refused for this.
pub fn plan_removal(
    members: &[Member],
    id: &str,
    leader_id: &str,
    reachable: impl Fn(&str) -> bool,
    force: bool,
) -> Result<Vec<Member>, RemovalRefused> {
    let Some(target) = members.iter().find(|member| member.id == id) else {
        return Err(RemovalRefused::NotMember);
    };
    if id == leader_id {
        return Err(RemovalRefused::Leader);
    }
    let next: Vec<Member> = members
        .iter()
        .filter(|member| member.id != id)
        .cloned()
        .collect();
    if target.role == Role::Voter && !force {
        let voters: Vec<&Member> = next
            .iter()
            .filter(|member| member.role == Role::Voter)
            .collect();
        let up = voters
            .iter()
            .filter(|voter| voter.id == leader_id || reachable(&voter.id))
            .count();
        if up < voters.len() / 2 + 1 {
            return Err(RemovalRefused::NoMajority {
                voters: voters.len(),
                reachable: up,
            });
        }
    }
    Ok(next)
}
