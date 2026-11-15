// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::sync::OnceLock;

use parking_lot::Mutex;
use pistack_store::Store;
use tokio::sync::broadcast;
use tokio::time::Instant;

use crate::proto::AgentEvent;

use super::nodes::State;
use super::queue_config::QueueConfig;

pub(crate) struct Inner {
    pub(crate) cfg: QueueConfig,
    pub(crate) state: Mutex<State>,
    pub(crate) agent_events: broadcast::Sender<AgentEvent>,
    /// Where keyed commands record their ids. Unset in tests and tools that
    /// run a bare queue; ids then last as long as the process.
    pub(crate) store: OnceLock<Store>,
    /// One keyed enqueue at a time, so the store is read and written
    /// outside the state lock without two callers minting an id each.
    pub(crate) keyed_lock: Mutex<()>,
    pub(crate) swept: Mutex<Option<Instant>>,
}
