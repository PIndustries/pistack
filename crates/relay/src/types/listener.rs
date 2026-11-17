// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A listener: the protocol and port on a balancer's VIP, the pool it
//! defaults to, and the door that publishes it off the host.

use super::balancer::default_true;
use super::kinds;
use super::route::Route;
use crate::door::DoorKind;
use pistack_types::{Id, Kind, Meta, Phase, Resource};
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListenerSpec {
    pub balancer: Id,
    pub protocol: ListenerProtocol,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_pool: Option<Id>,
    /// Max concurrent connections; 0 means unlimited.
    #[serde(default)]
    pub connection_limit: u32,
    #[serde(default = "default_true")]
    pub admin_up: bool,
    /// Publish this listener off the host. Absent means the listener is
    /// only the local forwarder. WireGuard may create only a `pw` interface.
    /// Edge queues a publish and starts no process on this server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub door: Option<DoorKind>,
    /// HTTP routes, tried before the default pool. A request goes to the
    /// route whose host matches most specifically, then the longest
    /// matching path. When no route matches, the default pool serves it;
    /// without a default member the answer is 404. Only `Http` and
    /// `Terminated` listeners may carry routes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<Route>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ListenerStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_ref: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}

resource!(
    /// A protocol and port on a balancer's VIP.
    Listener,
    ListenerSpec,
    ListenerStatus,
    kinds::LISTENER
);
