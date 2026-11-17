// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Whether one member may take datagrams: the picker's own eligibility rule,
//! so UDP and TCP agree on who is up.

use crate::types::Member;

pub(super) fn is_up(member: &Member) -> bool {
    crate::picker::eligible(std::slice::from_ref(member)).len() == 1
}
