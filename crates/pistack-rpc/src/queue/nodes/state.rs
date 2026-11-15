// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::collections::HashMap;

use crate::proto::CommandId;

use super::node_entry::NodeEntry;
use crate::queue::pending::Pending;

#[derive(Default)]
pub(crate) struct State {
    pub(crate) nodes: HashMap<String, NodeEntry>,
    pub(crate) pending: HashMap<CommandId, Pending>,
    /// Record key to the pending command that holds it.
    pub(crate) keyed: HashMap<String, CommandId>,
}
