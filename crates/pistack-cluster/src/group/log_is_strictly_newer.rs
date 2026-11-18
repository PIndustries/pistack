// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::node::Node;

/// `peer` is strictly newer when its last term is higher, or the terms match and its log is longer.
pub(crate) fn log_is_strictly_newer(peer: &Node, candidate: &Node) -> bool {
    let peer_term = peer.log.last().map(|entry| entry.term).unwrap_or(0);
    let candidate_term = candidate.log.last().map(|entry| entry.term).unwrap_or(0);
    peer_term > candidate_term
        || (peer_term == candidate_term && peer.log.len() > candidate.log.len())
}
