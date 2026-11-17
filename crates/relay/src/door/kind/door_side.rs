// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Store and command queue an edge door reads. Cloudflared, tailscale, and
//! WireGuard ignore it.

use pistack_rpc::CommandQueue;
use pistack_store::Store;

#[derive(Clone, Copy)]
pub struct DoorSide<'a> {
    pub store: &'a Store,
    pub queue: Option<&'a CommandQueue>,
}
