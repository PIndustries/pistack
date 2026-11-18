// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A fresh node classifies what it heard. A running node admits a join
//! without duplicating a member that is already present.

mod accepts_writes;
mod ballot;
mod check_follower_tip;
mod classify;
mod consider_pre_vote;
mod consider_vote;
mod election_seed;
mod election_timeout;
mod election_timeout_min;
mod election_timeout_span;
mod entries_after;
mod follow_candidate;
mod formation;
mod in_leader_lease;
mod is_voter;
mod join_membership;
mod join_request;
mod log_is_current;
mod lost_quorum;
mod peer_view;
mod pick_leader;
mod plan_promotions;
mod plan_removal;
mod removal_refused;
mod seed_membership;
mod should_campaign;
mod should_discover;
mod stored_entry;
mod stored_op;
#[cfg(test)]
mod tests;
mod vote_request;
mod vote_response;
mod voter_target;
mod with_agent_address;
pub use accepts_writes::accepts_writes;
pub use ballot::Ballot;
pub use check_follower_tip::check_follower_tip;
pub use classify::classify;
pub use consider_pre_vote::consider_pre_vote;
pub use consider_vote::consider_vote;
pub use election_seed::election_seed;
pub use election_timeout::election_timeout;
pub use election_timeout_min::ELECTION_TIMEOUT_MIN;
pub use election_timeout_span::ELECTION_TIMEOUT_SPAN;
pub use entries_after::entries_after;
pub use follow_candidate::follow_candidate;
pub use formation::Formation;
pub use in_leader_lease::in_leader_lease;
pub use is_voter::is_voter;
pub use join_membership::join_membership;
pub use join_request::JoinRequest;
pub use lost_quorum::lost_quorum;
pub use peer_view::PeerView;
pub use pick_leader::pick_leader;
pub use plan_promotions::plan_promotions;
pub use plan_removal::plan_removal;
pub use removal_refused::RemovalRefused;
pub use seed_membership::seed_membership;
pub use should_campaign::should_campaign;
pub use should_discover::should_discover;
pub use stored_entry::StoredEntry;
pub use stored_op::StoredOp;
pub use vote_request::VoteRequest;
pub use vote_response::VoteResponse;
pub use voter_target::voter_target;
pub use with_agent_address::with_agent_address;
