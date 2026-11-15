// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use pistack_types::Id;
pub(in crate::controllers::pod) fn host_id(raw: &str) -> Option<Id> {
    Id::parse(pistack_types::kind::HOST, raw)
}
