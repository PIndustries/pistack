// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;

use crate::proto::{AgentEvent, AgentHello, HelloAck, MAX_LONG_POLL_SECS};
use crate::queue::command_queue::CommandQueue;

impl CommandQueue {
    /// Record an agent's hello and return what to tell it.
    pub fn hello(&self, hello: &AgentHello) -> HelloAck {
        let now = Utc::now();
        {
            let mut st = self.0.state.lock();
            let e = self.entry(&mut st, &hello.node_name);
            e.info.capabilities = hello.capabilities.clone();
            e.info.labels = hello.labels.clone();
            e.info.version = hello.version.clone();
            e.info.underlay_ip = hello.underlay_ip.clone();
            e.info.resources_total = hello.resources;
            e.info.last_hello = Some(now);
        }
        tracing::info!(node = %hello.node_name, version = %hello.version, "agent hello");
        self.emit(AgentEvent::Hello(hello.clone()));
        HelloAck {
            node_name: hello.node_name.clone(),
            server_time: now,
            heartbeat_interval_secs: self.0.cfg.heartbeat_interval.as_secs().max(1),
            max_long_poll_secs: MAX_LONG_POLL_SECS,
            ovn: self.0.cfg.ovn.clone(),
        }
    }
}
