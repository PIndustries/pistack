// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_rpc::CommandQueue;
use pistack_store::Store;
/// A host reached through the command queue. One per node; the registrar
/// creates them as agents say hello.
pub struct RpcDriver {
    pub(super) queue: CommandQueue,
    pub(super) node: String,
    /// Used to resolve `spec.size` into `Resources` for the request. Without
    /// it the request carries zero resources, which the mock does not mind.
    pub(super) store: Option<Store>,
}
