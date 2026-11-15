// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

pub(in crate::backend::ovn) fn router_name(id: &Id) -> String {
    format!("pr-{id}")
}
