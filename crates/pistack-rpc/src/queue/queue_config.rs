// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use crate::proto::OvnHostConfig;

/// Tunables for [`CommandQueue`].
#[derive(Clone, Debug)]
pub struct QueueConfig {
    /// From enqueue to failure if no result arrives.
    pub command_timeout: Duration,
    /// Undelivered commands a node may have queued before `enqueue` fails.
    pub capacity: usize,
    /// Interval agents are told to heartbeat at.
    pub heartbeat_interval: Duration,
    /// OVN settings handed to every agent in the hello answer. `None`
    /// when the cluster's networks are not OVN.
    pub ovn: Option<OvnHostConfig>,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            command_timeout: Duration::from_secs(120),
            capacity: 1024,
            heartbeat_interval: Duration::from_secs(15),
            ovn: None,
        }
    }
}
