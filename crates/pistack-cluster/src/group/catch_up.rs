// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::has_index::has_index;
use super::node::Node;

/// Copy any entries and the commit index `follower` is missing.
pub fn catch_up(leader: &Node, follower: &mut Node) {
    for entry in &leader.log {
        if !has_index(follower, entry.index) {
            follower.log.push(entry.clone());
        }
    }
    if has_index(follower, leader.commit) {
        follower.commit = leader.commit;
    }
}
