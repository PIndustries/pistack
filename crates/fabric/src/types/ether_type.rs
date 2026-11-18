// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum EtherType {
    #[default]
    IPv4,
    IPv6,
}
