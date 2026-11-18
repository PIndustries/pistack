// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::ballot::Ballot;
use super::log_is_current::log_is_current;
use super::vote_request::VoteRequest;
use super::vote_response::VoteResponse;

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
