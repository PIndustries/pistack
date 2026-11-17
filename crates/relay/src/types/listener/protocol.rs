// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The protocol a listener serves on the balancer's VIP.

use pistack_types::Id;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ListenerProtocol {
    Tcp,
    Udp,
    Http,
    /// HTTPS passed through to members untouched (L4).
    Https,
    /// TLS terminated at the balancer with a `keep` certificate.
    Terminated {
        cert: Id,
    },
}

impl ListenerProtocol {
    /// True for protocols only the L7 provider can serve.
    pub fn is_l7(&self) -> bool {
        matches!(
            self,
            ListenerProtocol::Http | ListenerProtocol::Terminated { .. }
        )
    }
}
