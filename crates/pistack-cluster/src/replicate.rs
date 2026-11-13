// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A fresh node classifies what it heard. A running node admits a join
//! without duplicating a member that is already present.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::discover::Announce;
use crate::group::{ClusterError, Member, Role};

/// Shortest wait after the last sign of a leader or candidate before a
/// follower campaigns.
pub const ELECTION_TIMEOUT_MIN: Duration = Duration::from_millis(800);
/// Extra wait mixed in so two followers do not increment the same term.
pub const ELECTION_TIMEOUT_SPAN: Duration = Duration::from_millis(1_200);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinRequest {
    pub node_id: String,
    pub api: String,
    /// The joiner's address for host agents, when it has a TLS listener.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_api: Option<String>,
}

pub fn should_discover(existing_store: bool, init: bool, join: Option<&str>) -> bool {
    !existing_store && !init && join.map(str::trim).filter(|url| !url.is_empty()).is_none()
}

pub fn seed_membership(node_id: &str, api: &str, agent_api: Option<&str>) -> Vec<Member> {
    vec![Member::voter(node_id, api).with_agent_api(agent_api)]
}

/// A server that asks to join is a learner, new or not. A member that comes
/// back through join has a store it cannot vouch for (a downloaded snapshot
/// or a copy), so it is demoted and its addresses updated; no duplicate is
/// added. The leader promotes learners with [`plan_promotions`] once they
/// caught up.
pub fn join_membership(
    members: &[Member],
    node_id: &str,
    api: &str,
    agent_api: Option<&str>,
) -> Vec<Member> {
    let mut next = members.to_vec();
    match next.iter_mut().find(|member| member.id == node_id) {
        Some(member) => {
            member.api = api.to_owned();
            member.agent_api = agent_api.map(str::to_owned);
            member.role = Role::Learner;
        }
        None => next.push(Member::learner(node_id, api).with_agent_api(agent_api)),
    }
    next
}

/// The membership with `node_id`'s agent address set to `agent_api`, or
/// `None` when it already is or the server is not a member.
pub fn with_agent_address(
    members: &[Member],
    node_id: &str,
    agent_api: Option<&str>,
) -> Option<Vec<Member>> {
    let at = members.iter().position(|member| member.id == node_id)?;
    if members[at].agent_api.as_deref() == agent_api {
        return None;
    }
    let mut next = members.to_vec();
    next[at].agent_api = agent_api.map(str::to_owned);
    Some(next)
}

/// How many voters a membership should have: three once three servers are
/// members, otherwise the voters it already has (at least one). The second
/// server stays a learner, as [`crate::admit`] always did.
pub fn voter_target(members: &[Member]) -> usize {
    let voters = members
        .iter()
        .filter(|member| member.role == Role::Voter)
        .count();
    if members.len() >= 3 {
        3.max(voters)
    } else {
        voters.max(1)
    }
}

/// Learners to promote now, as the next membership, or `None`. Only learners
/// for which `caught_up` says yes are promoted, in member order, up to
/// [`voter_target`]. A step that would leave an even voter count short of
/// the target waits, so one voter and two learners become three voters
/// together rather than two voters that both must answer.
pub fn plan_promotions(
    members: &[Member],
    caught_up: impl Fn(&str) -> bool,
) -> Option<Vec<Member>> {
    let voters = members
        .iter()
        .filter(|member| member.role == Role::Voter)
        .count();
    let target = voter_target(members);
    if voters >= target {
        return None;
    }
    let ready: Vec<usize> = members
        .iter()
        .enumerate()
        .filter(|(_, member)| member.role == Role::Learner && caught_up(&member.id))
        .map(|(index, _)| index)
        .collect();
    let mut take = (target - voters).min(ready.len());
    if take > 0 && voters + take != target && (voters + take).is_multiple_of(2) {
        take -= 1;
    }
    if take == 0 {
        return None;
    }
    let mut next = members.to_vec();
    for index in ready.into_iter().take(take) {
        next[index].role = Role::Voter;
    }
    Some(next)
}

/// Why a server cannot be removed from the membership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemovalRefused {
    /// No member has that id.
    NotMember,
    /// The server asked to do the removal is the one to remove.
    Leader,
    /// Too few of the voters that would be left are answering.
    NoMajority { voters: usize, reachable: usize },
}

impl std::fmt::Display for RemovalRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotMember => f.write_str("no such server in this cluster"),
            Self::Leader => f.write_str(
                "this server is the leader and cannot remove itself: stop it, wait for another server to take over, and remove it from there",
            ),
            Self::NoMajority { voters, reachable } => write!(
                f,
                "removing this server would leave {voters} voters of which only {reachable} are answering, which is not a majority; pass force to remove it anyway"
            ),
        }
    }
}

/// The membership without `id`, or why it must stay.
///
/// `leader_id` is the server doing the removal and counts as reachable;
/// `reachable` says which other servers it heard from lately. Taking out a
/// voter is refused when fewer than a majority of the voters left are
/// reachable, because the cluster would stop committing the moment the
/// change is made. `force` skips that check and nothing else. A learner has
/// no vote, so removing one is never refused for this.
pub fn plan_removal(
    members: &[Member],
    id: &str,
    leader_id: &str,
    reachable: impl Fn(&str) -> bool,
    force: bool,
) -> Result<Vec<Member>, RemovalRefused> {
    let Some(target) = members.iter().find(|member| member.id == id) else {
        return Err(RemovalRefused::NotMember);
    };
    if id == leader_id {
        return Err(RemovalRefused::Leader);
    }
    let next: Vec<Member> = members
        .iter()
        .filter(|member| member.id != id)
        .cloned()
        .collect();
    if target.role == Role::Voter && !force {
        let voters: Vec<&Member> = next
            .iter()
            .filter(|member| member.role == Role::Voter)
            .collect();
        let up = voters
            .iter()
            .filter(|voter| voter.id == leader_id || reachable(&voter.id))
            .count();
        if up < voters.len() / 2 + 1 {
            return Err(RemovalRefused::NoMajority {
                voters: voters.len(),
                reachable: up,
            });
        }
    }
    Ok(next)
}

/// True when `id` is a voter in `members`.
pub fn is_voter(members: &[Member], id: &str) -> bool {
    members
        .iter()
        .any(|member| member.id == id && member.role == Role::Voter)
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
    /// Pre-vote: "would you vote for me at `term`?" Nothing is stored and no
    /// term moves. A candidate only raises its term after a majority says yes.
    #[serde(default)]
    pub pre: bool,
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
    let vote_free = match &local.voted_for {
        None => true,
        Some(choice) => choice == &req.candidate_id,
    };
    let granted = log_is_current(local, req) && vote_free;
    if granted {
        local.voted_for = Some(req.candidate_id.clone());
    }
    VoteResponse {
        term: local.current_term,
        granted,
    }
}

/// Pre-vote against the local ballot. Nothing changes. Yes only when the
/// proposed term is newer than ours and the candidate's log is at least as
/// current. Response.term is our current term.
pub fn consider_pre_vote(local: &Ballot, req: &VoteRequest) -> VoteResponse {
    VoteResponse {
        term: local.current_term,
        granted: req.term > local.current_term && log_is_current(local, req),
    }
}

fn log_is_current(local: &Ballot, req: &VoteRequest) -> bool {
    req.last_term > local.last_term
        || (req.last_term == local.last_term && req.last_index >= local.last_index)
}

/// True while a live leader was confirmed less than [`ELECTION_TIMEOUT_MIN`]
/// ago: a pull from it on a follower, a majority answer on the leader. Vote
/// and pre-vote requests are refused in that window and their term is not
/// adopted, so a node that lost touch cannot depose a healthy leader.
pub fn in_leader_lease(since_contact: Option<Duration>) -> bool {
    since_contact.is_some_and(|elapsed| elapsed < ELECTION_TIMEOUT_MIN)
}

/// A leader that has not heard from a voter majority for this long steps down.
pub fn lost_quorum(since_majority: Duration) -> bool {
    since_majority >= ELECTION_TIMEOUT_MIN + ELECTION_TIMEOUT_SPAN
}

/// What a peer said about itself on `/cluster`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerView {
    pub api: String,
    pub term: u64,
    pub leader: bool,
}

/// The peer to follow: one that says it leads, at our term or newer. The
/// newest term wins. A peer that leads an older term is stale.
pub fn pick_leader(term: u64, peers: &[PeerView]) -> Option<&PeerView> {
    peers
        .iter()
        .filter(|peer| peer.leader && peer.term >= term && !peer.api.is_empty())
        .max_by_key(|peer| peer.term)
}

/// Check a follower's tip before serving it entries after `index`.
/// `term` is the follower's term at `index` (0 when it has no row there),
/// `commit` is our commit index, and `ours` our term at `index`. A follower
/// past our commit, or whose term at `index` differs from ours, holds entries
/// this log never had. Error text contains "diverged".
pub fn check_follower_tip(
    index: u64,
    term: u64,
    commit: u64,
    ours: Option<u64>,
) -> Result<(), ClusterError> {
    if index > commit {
        return Err(ClusterError(format!(
            "diverged: follower is at index {index}, leader commit is {commit}"
        )));
    }
    match ours {
        Some(ours) if term != 0 && ours != term => Err(ClusterError(format!(
            "diverged at index {index}: follower term {term}, leader term {ours}"
        ))),
        _ => Ok(()),
    }
}

/// Mix node identity, attempt, and a clock so each election wait differs.
pub fn election_seed(node_id: &str, attempt: u64, now_nanos: u64) -> u64 {
    let mut seed = attempt
        .wrapping_mul(0x9E3779B97F4A7C15)
        .wrapping_add(now_nanos);
    for byte in node_id.as_bytes() {
        seed = seed.wrapping_mul(16_777_619).wrapping_add(*byte as u64);
    }
    seed
}

/// Wait in `[ELECTION_TIMEOUT_MIN, ELECTION_TIMEOUT_MIN + SPAN)`.
pub fn election_timeout(seed: u64) -> Duration {
    let mut x = seed | 1;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    ELECTION_TIMEOUT_MIN + Duration::from_millis(x % ELECTION_TIMEOUT_SPAN.as_millis() as u64)
}

/// A follower campaigns when the leader has been silent this long.
pub fn should_campaign(silent: Duration, timeout: Duration) -> bool {
    silent >= timeout
}

/// After granting a vote, follow that candidate instead of the old leader.
pub fn follow_candidate(members: &[Member], candidate_id: &str) -> Option<String> {
    members
        .iter()
        .find(|member| member.id == candidate_id)
        .map(|member| member.api.clone())
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
    fn every_join_is_a_learner_and_a_returning_voter_is_demoted() {
        let members = seed_membership("a", "http://a", None);
        let two = join_membership(&members, "b", "http://b", None);
        assert_eq!(two[0], Member::voter("a", "http://a"));
        assert_eq!(two[1], Member::learner("b", "http://b"));
        let three = join_membership(&two, "c", "http://c", None);
        assert_eq!(three.len(), 3);
        assert_eq!(three[2], Member::learner("c", "http://c"));
        assert_eq!(
            three.iter().filter(|m| m.role == Role::Voter).count(),
            1,
            "a join never adds a voter by itself"
        );
        let voters = vec![
            Member::voter("a", "http://a"),
            Member::voter("b", "http://b"),
            Member::voter("c", "http://c"),
        ];
        let again = join_membership(&voters, "c", "http://c2", None);
        assert_eq!(again.len(), 3);
        assert_eq!(again[2], Member::learner("c", "http://c2"));
        assert_eq!(again[0].role, Role::Voter);
        assert_eq!(again[1].role, Role::Voter);
    }

    #[test]
    fn a_member_carries_its_agent_address_and_an_old_record_has_none() {
        let tls = Some("https://a:8444");
        let members = seed_membership("a", "http://a", tls);
        assert_eq!(members[0].agent_api.as_deref(), tls);
        let two = join_membership(&members, "b", "http://b", Some("https://b:8444"));
        assert_eq!(two[1].agent_api.as_deref(), Some("https://b:8444"));
        // Coming back through join without a TLS listener clears it.
        let back = join_membership(&two, "b", "http://b", None);
        assert_eq!(back[1].agent_api, None);
        assert_eq!(back[0].agent_api.as_deref(), tls);

        assert_eq!(with_agent_address(&two, "a", tls), None);
        assert_eq!(with_agent_address(&two, "nobody", tls), None);
        let moved = with_agent_address(&two, "a", Some("https://a:9444")).unwrap();
        assert_eq!(moved[0].agent_api.as_deref(), Some("https://a:9444"));
        assert_eq!(moved[0].role, Role::Voter);
        assert_eq!(moved[1], two[1]);

        // A record from a server older than the field loads, and a member
        // without the address is written as it always was.
        let old = r#"[{"id":"a","api":"http://a","role":"Voter"}]"#;
        let loaded: Vec<Member> = serde_json::from_str(old).unwrap();
        assert_eq!(loaded, vec![Member::voter("a", "http://a")]);
        assert_eq!(serde_json::to_string(&loaded).unwrap(), old);
        let new = serde_json::to_string(&members).unwrap();
        assert!(new.contains(r#""agent_api":"https://a:8444""#), "{new}");
    }

    #[test]
    fn learners_are_promoted_only_when_caught_up_and_never_to_an_even_pair() {
        let all = |_: &str| true;
        let none = |_: &str| false;
        // Two servers: the second stays a learner.
        let two = vec![Member::voter("a", "a"), Member::learner("b", "b")];
        assert!(plan_promotions(&two, all).is_none());
        // One voter, two learners: wait for both, then promote both.
        let three = vec![
            Member::voter("a", "a"),
            Member::learner("b", "b"),
            Member::learner("c", "c"),
        ];
        assert!(plan_promotions(&three, none).is_none());
        assert!(plan_promotions(&three, |id| id == "b").is_none());
        let next = plan_promotions(&three, all).unwrap();
        assert!(next.iter().all(|m| m.role == Role::Voter));
        // A rejoined voter: two voters and a caught-up learner make three.
        let rejoin = vec![
            Member::voter("a", "a"),
            Member::voter("b", "b"),
            Member::learner("c", "c"),
        ];
        assert!(plan_promotions(&rejoin, none).is_none());
        let next = plan_promotions(&rejoin, |id| id == "c").unwrap();
        assert!(next.iter().all(|m| m.role == Role::Voter));
        // Three voters already: a fourth server stays a learner.
        let mut four = next.clone();
        four.push(Member::learner("d", "d"));
        assert!(plan_promotions(&four, all).is_none());
        assert!(is_voter(&four, "a"));
        assert!(!is_voter(&four, "d"));
        assert!(!is_voter(&four, "zz"));
    }

    #[test]
    fn a_removal_needs_a_majority_of_the_voters_left_unless_forced() {
        let three = vec![
            Member::voter("a", "a"),
            Member::voter("b", "b"),
            Member::voter("c", "c"),
        ];
        let all = |_: &str| true;
        let none = |_: &str| false;
        // The dead one goes: the two left are both up.
        let next = plan_removal(&three, "c", "a", |id| id == "b", false).unwrap();
        assert_eq!(next.len(), 2);
        assert!(is_voter(&next, "a") && is_voter(&next, "b"));
        assert_eq!(voter_target(&next), 2);
        // The live one would go and leave the leader with the dead one.
        assert_eq!(
            plan_removal(&three, "b", "a", |id| id == "b", false),
            Err(RemovalRefused::NoMajority {
                voters: 2,
                reachable: 1
            })
        );
        let forced = plan_removal(&three, "b", "a", |id| id == "b", true).unwrap();
        assert!(!forced.iter().any(|m| m.id == "b"));
        // Force does not let the leader remove itself, or find a stranger.
        assert_eq!(
            plan_removal(&three, "a", "a", all, true),
            Err(RemovalRefused::Leader)
        );
        assert_eq!(
            plan_removal(&three, "zz", "a", all, true),
            Err(RemovalRefused::NotMember)
        );
        // A learner has no vote, so nothing about the majority changes.
        let mut four = three.clone();
        four.push(Member::learner("d", "d"));
        let next = plan_removal(&four, "d", "a", none, false).unwrap();
        assert_eq!(next, three);
        // With a learner waiting, the voter that went is replaced.
        let next = plan_removal(&four, "c", "a", all, false).unwrap();
        let promoted = plan_promotions(&next, all).unwrap();
        assert!(promoted.iter().all(|m| m.role == Role::Voter));
        assert_eq!(promoted.len(), 3);
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
                pre: false,
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
                pre: false,
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
                pre: false,
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
                pre: false,
            },
        );
        assert!(newer.granted);
        assert_eq!(newer.term, 5);
        assert_eq!(local.voted_for.as_deref(), Some("d"));
        assert_eq!(local.last_term, 2);
        assert_eq!(local.last_index, 3);
    }

    #[test]
    fn silence_starts_an_election_and_a_grant_follows_the_candidate() {
        let timeout = Duration::from_millis(800);
        assert!(!should_campaign(Duration::from_millis(799), timeout));
        assert!(should_campaign(timeout, timeout));
        let first = election_timeout(election_seed("sim2", 1, 1));
        let second = election_timeout(election_seed("sim3", 1, 1));
        assert_ne!(first, second);
        assert!(first >= ELECTION_TIMEOUT_MIN);
        assert!(second >= ELECTION_TIMEOUT_MIN);
        assert!(first < ELECTION_TIMEOUT_MIN + ELECTION_TIMEOUT_SPAN);
        assert!(second < ELECTION_TIMEOUT_MIN + ELECTION_TIMEOUT_SPAN);
        let members = vec![
            Member::voter("sim", "http://sim"),
            Member::voter("sim2", "http://sim2"),
            Member::voter("sim3", "http://sim3"),
        ];
        assert_eq!(
            follow_candidate(&members, "sim3").as_deref(),
            Some("http://sim3")
        );
        assert_eq!(follow_candidate(&members, "gone"), None);
    }

    #[test]
    fn a_pre_vote_changes_nothing_and_needs_a_newer_term_and_a_current_log() {
        let local = Ballot {
            current_term: 7,
            voted_for: Some("sim3".into()),
            last_index: 40,
            last_term: 7,
        };
        let ask = |term, last_index, last_term| VoteRequest {
            term,
            candidate_id: "sim".into(),
            last_index,
            last_term,
            pre: true,
        };
        let yes = consider_pre_vote(&local, &ask(8, 40, 7));
        assert!(yes.granted);
        assert_eq!(yes.term, 7);
        // The old leader came back with a log from term 6.
        assert!(!consider_pre_vote(&local, &ask(8, 99, 6)).granted);
        assert!(!consider_pre_vote(&local, &ask(7, 40, 7)).granted);
        assert!(!consider_pre_vote(&local, &ask(8, 39, 7)).granted);
        assert_eq!(local.current_term, 7);
        assert_eq!(local.voted_for.as_deref(), Some("sim3"));
        let old: VoteRequest = serde_json::from_str(
            r#"{"term":8,"candidate_id":"sim","last_index":40,"last_term":7}"#,
        )
        .unwrap();
        assert!(!old.pre);
    }

    #[test]
    fn a_recent_leader_holds_a_lease_and_a_lost_majority_ends_it() {
        assert!(!in_leader_lease(None));
        assert!(in_leader_lease(Some(Duration::from_millis(100))));
        assert!(!in_leader_lease(Some(ELECTION_TIMEOUT_MIN)));
        assert!(!lost_quorum(ELECTION_TIMEOUT_MIN));
        assert!(lost_quorum(ELECTION_TIMEOUT_MIN + ELECTION_TIMEOUT_SPAN));
    }

    #[test]
    fn follow_the_newest_leader_and_never_a_stale_one() {
        let view = |api: &str, term, leader| PeerView {
            api: api.into(),
            term,
            leader,
        };
        let peers = vec![
            view("http://sim", 6, true),
            view("http://sim2", 7, false),
            view("http://sim3", 7, true),
        ];
        assert_eq!(pick_leader(6, &peers).unwrap().api, "http://sim3");
        assert_eq!(pick_leader(7, &peers).unwrap().api, "http://sim3");
        assert!(pick_leader(8, &peers).is_none());
        assert!(pick_leader(0, &[view("http://sim", 6, true)]).is_some());
        assert!(pick_leader(7, &[view("http://sim", 6, true)]).is_none());
    }

    #[test]
    fn a_follower_tip_must_match_the_leader_log() {
        assert!(check_follower_tip(0, 0, 0, None).is_ok());
        assert!(check_follower_tip(5, 6, 9, Some(6)).is_ok());
        // Rows written before followers kept them cannot be compared.
        assert!(check_follower_tip(5, 0, 9, Some(6)).is_ok());
        assert!(check_follower_tip(5, 6, 9, None).is_ok());
        let ahead = check_follower_tip(10, 6, 9, None).unwrap_err();
        assert!(ahead.to_string().contains("diverged"), "{ahead}");
        let forked = check_follower_tip(9, 6, 12, Some(7)).unwrap_err();
        assert!(forked.to_string().contains("diverged"), "{forked}");
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
