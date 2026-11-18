// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::quorum::quorum;
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
