// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::cluster_error::ClusterError;
use super::entry::Entry;
use super::has_index::has_index;
use super::node::Node;
use super::quorum::quorum;
use super::role::Role;

/// Append `payload` on `leader` and replicate to `others`.
/// Commit when a voter majority has the entry. Learners copy it and do not vote.
pub fn propose(
    leader: &mut Node,
    others: &mut [Node],
    payload: Vec<u8>,
) -> Result<u64, ClusterError> {
    if !leader.leader {
        return Err(ClusterError("not the leader".into()));
    }
    let index = leader.log.last().map(|entry| entry.index + 1).unwrap_or(1);
    let entry = Entry {
        index,
        term: leader.term,
        payload,
    };
    leader.log.push(entry.clone());
    for node in others.iter_mut() {
        if !has_index(node, index) {
            node.log.push(entry.clone());
        }
    }
    let voting = 1 + others
        .iter()
        .filter(|node| node.role == Role::Voter)
        .count();
    let have = 1 + others
        .iter()
        .filter(|node| node.role == Role::Voter && has_index(node, index))
        .count();
    if have < quorum(voting) {
        return Err(ClusterError("no voter quorum".into()));
    }
    leader.commit = index;
    for node in others.iter_mut() {
        if has_index(node, index) {
            node.commit = index;
        }
    }
    Ok(index)
}
