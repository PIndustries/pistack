// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;

/// The overlay plan last sent to one host, and how that send went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OverlaySent {
    pub(crate) body: String,
    /// The host's hello time when it was sent. A new hello means the agent
    /// restarted, so the plan goes again.
    pub(crate) hello: Option<chrono::DateTime<Utc>>,
    /// Set when the agent answered with a failure or never answered.
    pub(crate) failed: Option<std::time::Instant>,
}
