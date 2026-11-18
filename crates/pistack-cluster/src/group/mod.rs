// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Membership and a small voter log.
//!
//! One voter commits alone. A second node is a learner and copies the log.
//! When a third node joins, every current member becomes a voter. Later
//! nodes stay learners. A learner cannot win an election.

mod admit;
mod campaign;
mod catch_up;
mod cluster_error;
mod entry;
mod has_index;
mod log_is_strictly_newer;
mod member;
mod node;
mod propose;
mod quorum;
mod role;
#[cfg(test)]
mod tests;
pub use admit::admit;
pub use campaign::campaign;
pub use catch_up::catch_up;
pub use cluster_error::ClusterError;
pub use member::Member;
pub use node::Node;
pub use propose::propose;
pub use quorum::quorum;
pub use role::Role;
