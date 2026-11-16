// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// Capacity of the [`AgentEvent`] broadcast; a subscriber this far behind
/// sees `Lagged` and should resync from [`CommandQueue::nodes`].
pub const AGENT_EVENT_CAPACITY: usize = 1024;

/// Meta keys under this prefix hold the ids of keyed commands, as
/// `<prefix><node>/<key>`.
pub const COMMAND_RECORD_PREFIX: &str = "rpc/commands/";

/// How often a queue operation also looks for records nothing in this
/// process owns: what an earlier leader left for commands nobody issued
/// again.
pub(super) const SWEEP_EVERY: Duration = Duration::from_secs(30);
