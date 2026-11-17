// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP reverse proxy. Serves [`Provider::Relay`].

use super::slot::Slot;
use crate::provider::ListenerView;
use pistack_types::{ApiError, ApiResult, Id};
use std::collections::HashMap;

pub struct RelayL7Provider {
    pub(super) slots: parking_lot::Mutex<HashMap<Id, Slot>>,
}

impl Default for RelayL7Provider {
    fn default() -> Self {
        Self {
            slots: parking_lot::Mutex::new(HashMap::new()),
        }
    }
}

impl RelayL7Provider {
    pub fn new() -> Self {
        Self::default()
    }

    pub(super) async fn stop(&self, id: &Id) {
        let slot = self.slots.lock().remove(id);
        if let Some(mut slot) = slot {
            slot.cancel.cancel();
            if let Some(udp) = slot.udp.take() {
                udp.shutdown().await;
            }
            if let Some(task) = slot.task {
                let _ = task.await;
            }
        }
    }

    pub(super) async fn ensure_udp(&self, view: &ListenerView) -> ApiResult<String> {
        let targets = crate::udp::targets(view);
        let id = view.balancer.meta.id.clone();
        if targets.is_empty() {
            if let Some(slot) = self.slots.lock().get(&id) {
                if let Some(udp) = &slot.udp {
                    udp.set_members(Vec::new());
                }
            }
            return Err(ApiError::unavailable("udp pool has no up member"));
        }
        let ip = {
            let slots = self.slots.lock();
            let slot = slots
                .get(&id)
                .ok_or_else(|| ApiError::bad_request("http balancer is not bound"))?;
            if let Some(udp) = &slot.udp {
                udp.set_members(targets);
                return Ok(udp.bound().to_string());
            }
            slot.bound.ip()
        };
        let udp = crate::udp::UdpForward::bind(std::net::SocketAddr::new(ip, 0), targets).await?;
        let bound = udp.bound().to_string();
        let mut slots = self.slots.lock();
        let slot = slots
            .get_mut(&id)
            .ok_or_else(|| ApiError::bad_request("http balancer is not bound"))?;
        if let Some(existing) = &slot.udp {
            existing.set_members(crate::udp::targets(view));
            drop(udp);
            return Ok(existing.bound().to_string());
        }
        slot.udp = Some(udp);
        Ok(bound)
    }
}

impl Drop for RelayL7Provider {
    fn drop(&mut self) {
        let slots = std::mem::take(&mut *self.slots.lock());
        for (_, slot) in slots {
            slot.cancel.cancel();
            if let Some(task) = slot.task {
                task.abort();
            }
        }
    }
}
