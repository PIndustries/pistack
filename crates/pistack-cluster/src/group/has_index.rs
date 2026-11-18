// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::node::Node;

pub(crate) fn has_index(node: &Node, index: u64) -> bool {
    node.log.iter().any(|entry| entry.index == index)
}
