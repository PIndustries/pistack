// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::group::Role;

pub fn accepts_writes(role: Role, leader: bool) -> bool {
    leader && matches!(role, Role::Voter)
}
