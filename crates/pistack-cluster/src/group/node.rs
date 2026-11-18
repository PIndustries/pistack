// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::entry::Entry;
use super::role::Role;

#[derive(Debug, Clone)]
pub struct Node {
    pub id: String,
    pub role: Role,
    pub term: u64,
    pub voted_for: Option<String>,
    pub log: Vec<Entry>,
    pub commit: u64,
    pub leader: bool,
}

impl Node {
    pub fn voter(id: &str) -> Self {
        Self::new(id, Role::Voter)
    }

    pub fn learner(id: &str) -> Self {
        Self::new(id, Role::Learner)
    }

    pub(crate) fn new(id: &str, role: Role) -> Self {
        Self {
            id: id.into(),
            role,
            term: 0,
            voted_for: None,
            log: Vec::new(),
            commit: 0,
            leader: false,
        }
    }
}
