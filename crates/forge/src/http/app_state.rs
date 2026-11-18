// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::driver::DriverRegistry;
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub drivers: Arc<DriverRegistry>,
    /// Treat unauthenticated requests as cloud admin.
    pub dev_admin: bool,
    /// Agent transport. Absent in dev mode; live migration is refused then.
    pub queue: Option<CommandQueue>,
    /// How a drain picks destinations. The reconciler's own strategy.
    pub strategy: crate::scheduler::Strategy,
    /// The window host reports land in, which stats are read from.
    pub samples: Arc<gauge::SampleRing>,
    /// Powers a host off and confirms it before its machines are evacuated.
    pub fencer: Arc<dyn crate::maintain::Fencer>,
}
