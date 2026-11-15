// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::consts::AGENT_LABEL;
use super::registrar::Registrar;
use crate::types::{new_host, usable_underlay, Host};
use chrono::Utc;
use pistack_rpc::proto::AgentHello;
use pistack_store::StoreError;
use pistack_types::Phase;
use std::collections::BTreeMap;
impl Registrar {
    /// Create or refresh the host for a hello and register its driver.
    pub fn on_hello(&self, h: &AgentHello) -> anyhow::Result<Host> {
        let col = self.store.collection::<Host>();
        let now = Utc::now();
        let underlay = usable_underlay(h.underlay_ip.as_deref());
        let host = match self.host_for_node(&h.node_name)? {
            Some(existing) => col.modify(&existing.meta.id, |x| {
                x.spec.capacity = h.resources;
                x.spec.drivers = h.capabilities.clone();
                x.spec.underlay_ip = underlay.clone();
                for (k, v) in &h.labels {
                    x.meta.labels.insert(k.clone(), v.clone());
                }
                x.meta.labels.insert(AGENT_LABEL.into(), "true".into());
                if !h.version.is_empty() {
                    x.meta
                        .annotations
                        .insert("pistack.io/agent-version".into(), h.version.clone());
                }
                x.status.phase = Phase::Ready;
                x.status.message.clear();
                x.status.last_seen = Some(now);
                Ok(())
            })?,
            None => {
                let mut labels: BTreeMap<String, String> = h.labels.clone();
                labels.insert(AGENT_LABEL.into(), "true".into());
                let mut host = new_host(
                    &h.node_name,
                    &h.node_name,
                    h.resources,
                    h.capabilities.clone(),
                    labels,
                );
                host.spec.underlay_ip = underlay;
                if !h.version.is_empty() {
                    host.meta
                        .annotations
                        .insert("pistack.io/agent-version".into(), h.version.clone());
                }
                match col.create(host) {
                    Ok(h) => h,
                    // Raced with another hello for the same node.
                    Err(StoreError::Conflict(_)) => self
                        .host_for_node(&h.node_name)?
                        .ok_or_else(|| anyhow::anyhow!("host {} vanished", h.node_name))?,
                    Err(e) => return Err(e.into()),
                }
            }
        };
        self.register(&host);
        tracing::info!(
            host = %host.meta.id,
            node = %h.node_name,
            drivers = ?h.capabilities,
            capacity = ?h.resources,
            "forge: host registered from agent hello"
        );
        Ok(host)
    }
}
