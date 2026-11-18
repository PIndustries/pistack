// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiResult, Id, Meta};
use relay::{Client, MemberSpec};
use std::net::SocketAddr;

use crate::realizer::member_slug::member_slug;

/// Make `pool`'s members exactly `members`. True when anything changed.
pub(crate) fn sync_members(
    client: &Client,
    project: &Option<Id>,
    prefix: &str,
    pool: &Id,
    members: &[SocketAddr],
) -> ApiResult<bool> {
    let have = client.members_of(pool)?;
    let mut changed = false;
    for member in &have {
        if !members.contains(&member.addr()) {
            client.delete_member(&member.meta.id)?;
            changed = true;
        }
    }
    for want in members {
        if !have.iter().any(|member| member.addr() == *want) {
            client.create_member(
                Meta::new(
                    relay::types::kinds::MEMBER,
                    format!("{prefix}-{}", member_slug(want)),
                    project.clone(),
                ),
                MemberSpec {
                    pool: pool.clone(),
                    address: want.ip(),
                    port: want.port(),
                    weight: 1,
                    backup: false,
                    admin_up: true,
                },
            )?;
            changed = true;
        }
    }
    Ok(changed)
}
