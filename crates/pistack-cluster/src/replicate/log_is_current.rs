// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::ballot::Ballot;
use super::vote_request::VoteRequest;

pub(crate) fn log_is_current(local: &Ballot, req: &VoteRequest) -> bool {
    req.last_term > local.last_term
        || (req.last_term == local.last_term && req.last_index >= local.last_index)
}
