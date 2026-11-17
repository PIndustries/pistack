// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! What kind of publisher a listener has, and what an edge door is allowed
//! to read.

use pistack_rpc::CommandQueue;
use pistack_store::Store;
use serde::{Deserialize, Serialize};

/// Store and command queue an edge door reads. Cloudflared, tailscale, and
/// WireGuard ignore it.
#[derive(Clone, Copy)]
pub struct DoorSide<'a> {
    pub store: &'a Store,
    pub queue: Option<&'a CommandQueue>,
}

/// Which publisher carries the listener.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoorKind {
    Cloudflared,
    Tailscale,
    Wireguard,
    Edge,
}
