// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::cluster_error::ClusterError;
use super::log_is_strictly_newer::log_is_strictly_newer;
use super::node::Node;
use super::quorum::quorum;
use super::role::Role;

/// `candidate` asks `voters` for a new term. A learner returns an error.
/// A candidate that already has a voter majority becomes leader.
pub fn campaign(candidate: &mut Node, voters: &mut [Node]) -> Result<(), ClusterError> {
    if candidate.role == Role::Learner {
        return Err(ClusterError("learner cannot lead".into()));
    }
    candidate.term += 1;
    candidate.voted_for = Some(candidate.id.clone());
    candidate.leader = false;
    let mut votes = 1;
    for peer in voters.iter_mut() {
        let voted_other = peer.term == candidate.term
            && peer
                .voted_for
                .as_ref()
                .is_some_and(|choice| choice != &candidate.id);
        if log_is_strictly_newer(peer, candidate) || voted_other {
            continue;
        }
        peer.term = candidate.term;
        peer.voted_for = Some(candidate.id.clone());
        peer.leader = false;
        votes += 1;
    }
    if votes >= quorum(1 + voters.len()) {
        candidate.leader = true;
        Ok(())
    } else {
        Err(ClusterError("election lost quorum".into()))
    }
}
