// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

/// One port of a service. `protocol` is `TCP`, `UDP`, or `SCTP`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ServicePortView {
    #[serde(default)]
    pub protocol: String,
    pub port: u16,
    #[serde(default)]
    pub target_port: Option<u16>,
}
