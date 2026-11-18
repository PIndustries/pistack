// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Machine;

use super::stats_type::Stats;

/// The current series of one machine.
pub fn machine_stats(gauge: &gauge::Client, machine: &Machine) -> Stats {
    let id = machine.meta.id.to_string();
    let series = gauge.stats(|s| s.labels.get("machine") == Some(&id));
    Stats {
        host: series
            .first()
            .and_then(|s| s.labels.get("host").cloned())
            .unwrap_or_default(),
        id,
        name: machine.meta.name.clone(),
        series,
    }
}
