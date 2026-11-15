// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::driver::DriverRegistry;
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use pistack_types::Id;
use std::collections::BTreeSet;
use std::sync::Arc;
/// Turns agent events into `Host` resources and registered drivers.
pub struct Registrar {
    pub(super) store: Store,
    pub(super) drivers: Arc<DriverRegistry>,
    pub(super) queue: CommandQueue,
    /// Where a heartbeat's samples go.
    pub(super) gauge: gauge::Client,
    /// Machines with a delete of a stale copy in flight, so a host that
    /// heartbeats before it answers is not asked twice.
    pub(super) deleting_stale: Arc<parking_lot::Mutex<BTreeSet<Id>>>,
}
