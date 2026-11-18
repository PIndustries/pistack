// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::driver::DriverRegistry;
use crate::scheduler::Strategy;
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use std::sync::Arc;

use super::overlay_sent_map::OverlaySentMap;

pub struct Reconciler {
    pub(super) store: Store,
    pub(super) drivers: Arc<DriverRegistry>,
    pub(super) strategy: Strategy,
    pub(super) queue: Option<CommandQueue>,
    /// Last overlay document sent to each node. A changed plan, a new agent
    /// hello, or a failed apply sends it again.
    pub(super) overlay_sent: OverlaySentMap,
}
