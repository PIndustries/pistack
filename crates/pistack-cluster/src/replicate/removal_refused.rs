// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

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
