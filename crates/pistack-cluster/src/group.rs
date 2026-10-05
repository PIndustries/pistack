// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Membership and a small voter log.
//!
//! One voter commits alone. A second node is a learner and copies the log.
//! When a third node joins, every current member becomes a voter. Later
//! nodes stay learners. A learner cannot win an election.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Voter,
    Learner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub id: String,
    pub api: String,
    pub role: Role,
}

impl Member {
    pub fn voter(id: &str, api: &str) -> Self {
        Self {
            id: id.into(),
            api: api.into(),
            role: Role::Voter,
        }
    }

    pub fn learner(id: &str, api: &str) -> Self {
        Self {
            id: id.into(),
            api: api.into(),
            role: Role::Learner,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub index: u64,
    pub term: u64,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: String,
    pub role: Role,
    pub term: u64,
    pub voted_for: Option<String>,
    pub log: Vec<Entry>,
    pub commit: u64,
    pub leader: bool,
}

impl Node {
    pub fn voter(id: &str) -> Self {
        Self::new(id, Role::Voter)
    }

    pub fn learner(id: &str) -> Self {
        Self::new(id, Role::Learner)
    }

    fn new(id: &str, role: Role) -> Self {
        Self {
            id: id.into(),
            role,
            term: 0,
            voted_for: None,
            log: Vec::new(),
            commit: 0,
            leader: false,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ClusterError(pub String);

impl fmt::Display for ClusterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Assign the joining node's role and return the new membership.
/// The caller's `role` on `newbie` is ignored.
pub fn admit(members: &[Member], mut newbie: Member) -> Vec<Member> {
    let voters = members
        .iter()
        .filter(|member| member.role == Role::Voter)
        .count();
    let learners = members
        .iter()
        .filter(|member| member.role == Role::Learner)
        .count();
    let promote = voters == 1 && learners >= 1;
    newbie.role = if voters >= 3 || (voters == 1 && learners == 0) {
        Role::Learner
    } else {
        Role::Voter
    };
    let mut next = members.to_vec();
    next.push(newbie);
    if promote {
        for member in &mut next {
            member.role = Role::Voter;
        }
    }
    next
}

/// Majority of `voters`, at least 1.
pub fn quorum(voters: usize) -> usize {
    voters / 2 + 1
}

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

fn has_index(node: &Node, index: u64) -> bool {
    node.log.iter().any(|entry| entry.index == index)
}

/// `peer` is strictly newer when its last term is higher, or the terms match and its log is longer.
fn log_is_strictly_newer(peer: &Node, candidate: &Node) -> bool {
    let peer_term = peer.log.last().map(|entry| entry.term).unwrap_or(0);
    let candidate_term = candidate.log.last().map(|entry| entry.term).unwrap_or(0);
    peer_term > candidate_term
        || (peer_term == candidate_term && peer.log.len() > candidate.log.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_second_node_learns_and_the_third_makes_voters() {
        let one = vec![Member::voter("a", "http://a")];
        let two = admit(&one, Member::voter("b", "http://b"));
        assert_eq!(two[0].role, Role::Voter);
        assert_eq!(two[1].role, Role::Learner);
        let three = admit(&two, Member::learner("c", "http://c"));
        assert!(three.iter().all(|m| m.role == Role::Voter));
        assert_eq!(three.len(), 3);
        let four = admit(&three, Member::voter("d", "http://d"));
        assert_eq!(four.iter().filter(|m| m.role == Role::Voter).count(), 3);
        assert_eq!(four[3].role, Role::Learner);
        assert_eq!(quorum(1), 1);
        assert_eq!(quorum(2), 2);
        assert_eq!(quorum(3), 2);
    }

    #[test]
    fn one_voter_commits_and_a_learner_copies() {
        let mut leader = Node::voter("a");
        leader.leader = true;
        let mut learner = Node::learner("b");
        let index = propose(
            &mut leader,
            std::slice::from_mut(&mut learner),
            b"one".to_vec(),
        )
        .unwrap();
        assert_eq!(index, 1);
        assert_eq!(leader.commit, 1);
        assert_eq!(learner.commit, 1);
        assert_eq!(learner.log, leader.log);
    }

    #[test]
    fn a_learner_cannot_take_over() {
        let mut leader = Node::voter("a");
        leader.leader = true;
        let mut learner = Node::learner("b");
        propose(
            &mut leader,
            std::slice::from_mut(&mut learner),
            b"one".to_vec(),
        )
        .unwrap();
        let err = campaign(&mut learner, &mut []).unwrap_err();
        assert!(err.0.contains("learner"));
        assert!(!learner.leader);
        assert_eq!(learner.commit, 1);
    }

    #[test]
    fn three_voters_survive_the_leader() {
        let mut a = Node::voter("a");
        a.leader = true;
        a.term = 1;
        let b = Node::voter("b");
        let c = Node::voter("c");
        propose(&mut a, &mut [b, c], b"one".to_vec()).unwrap();
        let mut b = Node::voter("b");
        let mut c = Node::voter("c");
        catch_up(&a, &mut b);
        catch_up(&a, &mut c);
        // `a` is not asked: the leader process is gone, not stepping down.
        campaign(&mut b, std::slice::from_mut(&mut c)).unwrap();
        assert!(b.leader);
        assert!(!c.leader);
        let index = propose(&mut b, std::slice::from_mut(&mut c), b"two".to_vec()).unwrap();
        assert_eq!(index, 2);
        assert_eq!(c.commit, 2);
        assert_eq!(c.log.last().unwrap().payload, b"two");
    }
}
