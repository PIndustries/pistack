// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::group::Member;

pub fn seed_membership(node_id: &str, api: &str, agent_api: Option<&str>) -> Vec<Member> {
    vec![Member::voter(node_id, api).with_agent_api(agent_api)]
}
