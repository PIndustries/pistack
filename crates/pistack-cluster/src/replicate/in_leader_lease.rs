// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use super::election_timeout_min::ELECTION_TIMEOUT_MIN;

/// True while a live leader was confirmed less than [`ELECTION_TIMEOUT_MIN`]
/// ago: a pull from it on a follower, a majority answer on the leader. Vote
/// and pre-vote requests are refused in that window and their term is not
/// adopted, so a node that lost touch cannot depose a healthy leader.
pub fn in_leader_lease(since_contact: Option<Duration>) -> bool {
    since_contact.is_some_and(|elapsed| elapsed < ELECTION_TIMEOUT_MIN)
}
