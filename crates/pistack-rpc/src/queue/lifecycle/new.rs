// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use tokio::sync::broadcast;

use crate::queue::command_queue::CommandQueue;
use crate::queue::consts::AGENT_EVENT_CAPACITY;
use crate::queue::inner::Inner;
use crate::queue::nodes::State;
use crate::queue::queue_config::QueueConfig;

impl CommandQueue {
    pub fn new(cfg: QueueConfig) -> Self {
        let (agent_events, _) = broadcast::channel(AGENT_EVENT_CAPACITY);
        Self(Arc::new(Inner {
            cfg,
            state: Mutex::new(State::default()),
            agent_events,
            store: OnceLock::new(),
            keyed_lock: Mutex::new(()),
            swept: Mutex::new(None),
        }))
    }
}
