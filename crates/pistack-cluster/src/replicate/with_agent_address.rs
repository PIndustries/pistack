// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::group::Member;

/// The membership with `node_id`'s agent address set to `agent_api`, or
/// `None` when it already is or the server is not a member.
pub fn with_agent_address(
    members: &[Member],
    node_id: &str,
    agent_api: Option<&str>,
) -> Option<Vec<Member>> {
    let at = members.iter().position(|member| member.id == node_id)?;
    if members[at].agent_api.as_deref() == agent_api {
        return None;
    }
    let mut next = members.to_vec();
    next[at].agent_api = agent_api.map(str::to_owned);
    Some(next)
}
