// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

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
