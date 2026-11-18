// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;

use super::stats_type::Stats;

/// The current series of one host: its own, not its machines'.
pub fn host_stats(gauge: &gauge::Client, host: &Host) -> Stats {
    let node = &host.spec.node_name;
    Stats {
        id: host.meta.id.to_string(),
        name: host.meta.name.clone(),
        host: node.clone(),
        series: gauge
            .stats(|s| s.labels.get("host") == Some(node) && !s.labels.contains_key("machine")),
    }
}
