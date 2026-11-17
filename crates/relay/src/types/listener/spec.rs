// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The listener's request: protocol and port, the pool it defaults to, and
//! the door that publishes it off the host.

use super::protocol::ListenerProtocol;
use crate::door::DoorKind;
use crate::types::default_true::default_true;
use crate::types::route::Route;
use pistack_types::Id;
use serde::{Deserialize, Serialize};

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
