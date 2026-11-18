// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

/// How a network is realised on the wire.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NetworkKind {
    /// VXLAN overlay. The segment id is the VNI.
    #[default]
    Overlay,
    /// Mapped onto a physical network, optionally VLAN-tagged.
    Provider {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vlan: Option<u16>,
        physnet: String,
    },
}
