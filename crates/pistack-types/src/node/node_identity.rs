// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `NodeIdentity`: the node name and how it was chosen.

use super::node_name_source::NodeNameSource;

/// The node name and how it was chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeIdentity {
    pub name: String,
    pub source: NodeNameSource,
    /// The name the file held before `PISTACK_NODE_NAME` replaced it.
    pub replaced: Option<String>,
}
