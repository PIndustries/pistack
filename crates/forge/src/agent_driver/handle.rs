// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::registrar::Registrar;
use pistack_rpc::proto::AgentEvent;
impl Registrar {
    /// Apply one event. Public so tests can drive it without the loop.
    pub fn handle(&self, ev: AgentEvent) -> anyhow::Result<()> {
        match ev {
            AgentEvent::Hello(h) => self.on_hello(&h).map(|_| ()),
            AgentEvent::Heartbeat(hb) => self.on_heartbeat(&hb),
        }
    }
}
