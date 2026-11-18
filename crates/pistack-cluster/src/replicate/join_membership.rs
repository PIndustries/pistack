// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::group::{Member, Role};

/// A server that asks to join is a learner, new or not. A member that comes
/// back through join has a store it cannot vouch for (a downloaded snapshot
/// or a copy), so it is demoted and its addresses updated; no duplicate is
/// added. The leader promotes learners with [`plan_promotions`] once they
/// caught up.
pub fn join_membership(
    members: &[Member],
    node_id: &str,
    api: &str,
    agent_api: Option<&str>,
) -> Vec<Member> {
    let mut next = members.to_vec();
    match next.iter_mut().find(|member| member.id == node_id) {
        Some(member) => {
            member.api = api.to_owned();
            member.agent_api = agent_api.map(str::to_owned);
            member.role = Role::Learner;
        }
        None => next.push(Member::learner(node_id, api).with_agent_api(agent_api)),
    }
    next
}
