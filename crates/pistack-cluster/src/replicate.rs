// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A fresh node classifies what it heard. A running node admits a join
//! without duplicating a member that is already present.

use serde::{Deserialize, Serialize};

use crate::discover::Announce;
use crate::group::{admit, ClusterError, Member, Role};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinRequest {
    pub node_id: String,
    pub api: String,
}

pub fn should_discover(existing_store: bool, init: bool, join: Option<&str>) -> bool {
    !existing_store && !init && join.map(str::trim).filter(|url| !url.is_empty()).is_none()
}

pub fn seed_membership(node_id: &str, api: &str) -> Vec<Member> {
    vec![Member::voter(node_id, api)]
}

/// Same rules as admit(), except a node id that is already a member keeps its
/// role and updates its api. Do not insert a duplicate.
pub fn join_membership(members: &[Member], node_id: &str, api: &str) -> Vec<Member> {
    if let Some(index) = members.iter().position(|member| member.id == node_id) {
        let mut next = members.to_vec();
        next[index].api = api.to_owned();
        return next;
    }
    admit(members, Member::learner(node_id, api))
}

pub fn accepts_writes(role: Role, leader: bool) -> bool {
    leader && matches!(role, Role::Voter)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Formation {
    /// No established cluster, and no forming peer with a smaller node id.
    Found,
    /// A forming peer (empty cluster_id) has a smaller node id.
    Wait,
    Join(Announce),
    Ambiguous(Vec<String>),
}

/// `heard` may include this node; ignore entries whose node_id == self_id.
/// An empty cluster_id means "still forming", not a cluster to join.
/// One distinct non-empty cluster_id -> Join the announce with the smallest node_id.
/// Two or more -> Ambiguous with the ids sorted.
/// None -> Wait if any remaining peer has a smaller node_id, otherwise Found.
pub fn classify(self_id: &str, heard: &[Announce]) -> Formation {
    let peers: Vec<&Announce> = heard.iter().filter(|ann| ann.node_id != self_id).collect();
    let mut clusters: Vec<String> = peers
        .iter()
        .filter(|ann| !ann.cluster_id.is_empty())
        .map(|ann| ann.cluster_id.clone())
        .collect();
    clusters.sort();
    clusters.dedup();
    match clusters.len() {
        0 => {
            if peers.iter().any(|ann| ann.node_id.as_str() < self_id) {
                Formation::Wait
            } else {
                Formation::Found
            }
        }
        1 => {
            let chosen = peers
                .iter()
                .copied()
                .filter(|ann| !ann.cluster_id.is_empty())
                .min_by(|left, right| left.node_id.cmp(&right.node_id))
                .expect("one cluster id has a peer");
            Formation::Join(chosen.clone())
        }
        _ => Formation::Ambiguous(clusters),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ballot {
    pub current_term: u64,
    pub voted_for: Option<String>,
    pub last_index: u64,
    pub last_term: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoteRequest {
    pub term: u64,
    pub candidate_id: String,
    pub last_index: u64,
    pub last_term: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoteResponse {
    pub term: u64,
    pub granted: bool,
}

/// Raft vote against the local ballot.
/// - req.term < local.current_term: deny, do not change local.
/// - req.term > local.current_term: set current_term, clear voted_for.
/// - Log is acceptable when req.last_term > local.last_term, or the terms
///   are equal and req.last_index >= local.last_index.
/// - Grant only when the log is acceptable AND voted_for is None or already
///   this candidate. On grant, set voted_for to the candidate.
///
/// Response.term is local.current_term after the update.
pub fn consider_vote(local: &mut Ballot, req: &VoteRequest) -> VoteResponse {
    if req.term < local.current_term {
        return VoteResponse {
            term: local.current_term,
            granted: false,
        };
    }
    if req.term > local.current_term {
        local.current_term = req.term;
        local.voted_for = None;
    }
    let log_ok = req.last_term > local.last_term
        || (req.last_term == local.last_term && req.last_index >= local.last_index);
    let vote_free = match &local.voted_for {
        None => true,
        Some(choice) => choice == &req.candidate_id,
    };
    let granted = log_ok && vote_free;
    if granted {
        local.voted_for = Some(req.candidate_id.clone());
    }
    VoteResponse {
        term: local.current_term,
        granted,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredOp {
    pub table: String,
    pub key: String,
    /// None deletes the key.
    pub value: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredEntry {
    pub index: u64,
    pub term: u64,
    pub ops: Vec<StoredOp>,
}

/// Entries with index > have, sorted by index. Error if the first is not
/// have+1 or if two consecutive indexes are not adjacent. Error text contains "gap".
pub fn entries_after(have: u64, log: &[StoredEntry]) -> Result<Vec<StoredEntry>, ClusterError> {
    let mut next: Vec<StoredEntry> = log
        .iter()
        .filter(|entry| entry.index > have)
        .cloned()
        .collect();
    next.sort_by_key(|entry| entry.index);
    let mut previous = have;
    for entry in &next {
        if entry.index != previous + 1 {
            return Err(ClusterError("gap".into()));
        }
        previous = entry.index;
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ann(cluster: &str, node: &str) -> Announce {
        Announce {
            cluster_id: cluster.into(),
            node_id: node.into(),
            api: format!("http://{node}"),
            raft: format!("{node}:18446"),
        }
    }

    fn entry(index: u64) -> StoredEntry {
        StoredEntry {
            index,
            term: 1,
            ops: Vec::new(),
        }
    }

    #[test]
    fn should_discover_only_without_a_store_init_or_join() {
        assert!(!should_discover(true, false, None));
        assert!(!should_discover(false, true, None));
        assert!(!should_discover(false, false, Some(" http://x ")));
        assert!(should_discover(false, false, None));
        assert!(should_discover(false, false, Some("  ")));
    }

    #[test]
    fn seed_then_join_follows_admit_and_updates_api() {
        let members = seed_membership("a", "http://a");
        let two = join_membership(&members, "b", "http://b");
        assert_eq!(two[0], Member::voter("a", "http://a"));
        assert_eq!(two[1], Member::learner("b", "http://b"));
        let three = join_membership(&two, "c", "http://c");
        assert_eq!(three.len(), 3);
        assert!(three.iter().all(|member| member.role == Role::Voter));
        let again = join_membership(&three, "b", "http://b2");
        assert_eq!(again.len(), 3);
        assert_eq!(again[0].role, Role::Voter);
        assert_eq!(again[1].id, "b");
        assert_eq!(again[1].role, Role::Voter);
        assert_eq!(again[1].api, "http://b2");
        assert_eq!(again[2].id, "c");
        assert_eq!(again[2].role, Role::Voter);
    }

    #[test]
    fn only_a_leading_voter_accepts_writes() {
        assert!(accepts_writes(Role::Voter, true));
        assert!(!accepts_writes(Role::Voter, false));
        assert!(!accepts_writes(Role::Learner, true));
    }

    #[test]
    fn classify_heard_peers() {
        assert_eq!(classify("m", &[]), Formation::Found);
        assert_eq!(classify("m", &[ann("", "a")]), Formation::Wait);
        assert_eq!(classify("m", &[ann("", "z")]), Formation::Found);
        match classify("m", &[ann("c1", "b"), ann("c1", "a")]) {
            Formation::Join(peer) => assert_eq!(peer.node_id, "a"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            classify("m", &[ann("c2", "b"), ann("c1", "a")]),
            Formation::Ambiguous(vec!["c1".into(), "c2".into()])
        );
        assert_eq!(classify("m", &[ann("c1", "m")]), Formation::Found);
        match classify("m", &[ann("c1", "m"), ann("c9", "b"), ann("", "a")]) {
            Formation::Join(peer) => assert_eq!(peer.node_id, "b"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn consider_vote_grants_once_per_term_and_rejects_an_older_log() {
        let mut local = Ballot {
            current_term: 0,
            voted_for: None,
            last_index: 0,
            last_term: 0,
        };
        let first = consider_vote(
            &mut local,
            &VoteRequest {
                term: 0,
                candidate_id: "a".into(),
                last_index: 0,
                last_term: 0,
            },
        );
        assert!(first.granted);
        assert_eq!(first.term, 0);
        assert_eq!(local.voted_for.as_deref(), Some("a"));
        let second = consider_vote(
            &mut local,
            &VoteRequest {
                term: 0,
                candidate_id: "b".into(),
                last_index: 0,
                last_term: 0,
            },
        );
        assert!(!second.granted);
        assert_eq!(local.voted_for.as_deref(), Some("a"));

        let mut local = Ballot {
            current_term: 1,
            voted_for: Some("a".into()),
            last_index: 3,
            last_term: 2,
        };
        let older = consider_vote(
            &mut local,
            &VoteRequest {
                term: 4,
                candidate_id: "c".into(),
                last_index: 0,
                last_term: 0,
            },
        );
        assert!(!older.granted);
        assert_eq!(older.term, 4);
        assert!(local.voted_for.is_none());

        let newer = consider_vote(
            &mut local,
            &VoteRequest {
                term: 5,
                candidate_id: "d".into(),
                last_index: 4,
                last_term: 3,
            },
        );
        assert!(newer.granted);
        assert_eq!(newer.term, 5);
        assert_eq!(local.voted_for.as_deref(), Some("d"));
        assert_eq!(local.last_term, 2);
        assert_eq!(local.last_index, 3);
    }

    #[test]
    fn entries_after_returns_the_suffix_or_a_gap() {
        let log = vec![entry(1), entry(2)];
        let got = entries_after(0, &log).unwrap();
        assert_eq!(got, log);
        let err = entries_after(1, &[entry(3)]).unwrap_err();
        assert!(err.to_string().contains("gap"), "{err}");
    }
}
