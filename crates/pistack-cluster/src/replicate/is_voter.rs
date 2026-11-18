// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::group::{Member, Role};

/// True when `id` is a voter in `members`.
pub fn is_voter(members: &[Member], id: &str) -> bool {
    members
        .iter()
        .any(|member| member.id == id && member.role == Role::Voter)
}
