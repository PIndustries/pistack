// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_rpc::CommandQueue;
use pistack_store::Store;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    /// Treat unauthenticated requests as cloud admin.
    pub dev_admin: bool,
    /// `None` until the server supplies the command queue.
    pub queue: Option<CommandQueue>,
}
