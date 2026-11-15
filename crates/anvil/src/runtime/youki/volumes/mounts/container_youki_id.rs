// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

pub(in crate::runtime::youki) fn container_youki_id(machine: &Id, index: usize) -> String {
    format!("{machine}-{index}")
}
