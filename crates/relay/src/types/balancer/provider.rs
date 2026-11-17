// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Which data plane realises the balancer.

use serde::{Deserialize, Serialize};

/// Which data plane realises the balancer.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// L4 TCP and UDP. The in-process forwarder serves this value. The default.
    /// Older rows and documents that say `ovn` read as this value.
    #[default]
    #[serde(alias = "ovn")]
    Forward,
    /// HTTP. The in-process reverse proxy serves this value.
    Relay,
}
