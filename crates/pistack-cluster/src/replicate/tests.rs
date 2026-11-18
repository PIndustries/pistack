// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use crate::discover::Announce;
use crate::group::{Member, Role};

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
    let old: VoteRequest =
        serde_json::from_str(r#"{"term":8,"candidate_id":"sim","last_index":40,"last_term":7}"#)
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
