// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::ballot::Ballot;
use super::log_is_current::log_is_current;
use super::vote_request::VoteRequest;
use super::vote_response::VoteResponse;

/// Pre-vote against the local ballot. Nothing changes. Yes only when the
/// proposed term is newer than ours and the candidate's log is at least as
/// current. Response.term is our current term.
pub fn consider_pre_vote(local: &Ballot, req: &VoteRequest) -> VoteResponse {
    VoteResponse {
        term: local.current_term,
        granted: req.term > local.current_term && log_is_current(local, req),
    }
}
