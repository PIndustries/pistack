// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;

use crate::proto::{AgentEvent, Heartbeat, HeartbeatAck};
use crate::queue::command_queue::CommandQueue;

impl CommandQueue {
    /// Record a heartbeat.
    pub fn heartbeat(&self, hb: &Heartbeat) -> HeartbeatAck {
        self.reap();
        let now = Utc::now();
        let mut st = self.0.state.lock();
        let e = self.entry(&mut st, &hb.node_name);
        e.info.resources_free = hb.resources_free;
        e.info.machines = hb.machines.clone();
        e.info.last_heartbeat = Some(now);
        let known = e.info.last_hello.is_some();
        let commands_waiting = self.0.cfg.capacity - e.tx.capacity();
        drop(st);
        self.emit(AgentEvent::Heartbeat(hb.clone()));
        HeartbeatAck {
            server_time: now,
            commands_waiting,
            known,
            servers: Vec::new(),
        }
    }
}
