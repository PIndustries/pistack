// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::discover::Announce;

#[derive(Debug, PartialEq, Eq)]
pub enum Formation {
    /// No established cluster, and no forming peer with a smaller node id.
    Found,
    /// A forming peer (empty cluster_id) has a smaller node id.
    Wait,
    Join(Announce),
    Ambiguous(Vec<String>),
}
