// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

/// `GET /compute/machines/{id}/stats` and `GET /compute/hosts/{id}/stats`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Stats {
    /// Machine or host id.
    pub id: String,
    pub name: String,
    /// The host's node name; for a machine, the host it reports from.
    /// Empty when the machine has no series.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host: String,
    /// Empty when nothing was reported in the last five minutes: the
    /// machine is off, its runtime does not measure, or its host is down.
    pub series: Vec<gauge::Stat>,
}
