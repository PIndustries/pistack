// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `LbProvider` surface: what a caller asks the forwarder to bind,
//! serve, and release.

use super::accept::accept_loop;
use super::bind::bind_target;
use super::provider::{dial_for, ForwardProvider};
use super::slot::Slot;
use crate::provider::{LbProvider, ListenerView, Realized};
use crate::types::{Balancer, ListenerProtocol, Provider};
use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult, Id};
use tokio::net::TcpListener;

fn realized(bound: std::net::SocketAddr) -> Realized {
    Realized {
        vip: bound.ip(),
        provider_ref: bound.to_string(),
    }
}

#[async_trait]
impl LbProvider for ForwardProvider {
    fn name(&self) -> &'static str {
        "forward"
    }

    fn serves(&self, provider: Provider) -> bool {
        provider == Provider::Forward
    }

    async fn ensure_balancer(&self, lb: &Balancer) -> ApiResult<Realized> {
        if let Some(bound) = self.slots.lock().get(&lb.meta.id).map(|s| s.bound) {
            return Ok(realized(bound));
        }
        let target = bind_target(lb)?;
        // A fixed port is bound by the listener that uses it, so a udp
        // listener never holds the tcp port (or the other way round).
        let (listener, bound) = if target.port() != 0 {
            (None, target)
        } else {
            let listener = TcpListener::bind(target)
                .await
                .map_err(|e| ApiError::unavailable(format!("forward bind {target}: {e}")))?;
            let bound = listener
                .local_addr()
                .map_err(|e| ApiError::unavailable(format!("forward local address: {e}")))?;
            (Some(listener), bound)
        };
        let mut slots = self.slots.lock();
        if let Some(existing) = slots.get(&lb.meta.id) {
            return Ok(realized(existing.bound));
        }
        slots.insert(lb.meta.id.clone(), Slot::new(bound, listener));
        Ok(realized(bound))
    }

    async fn delete_balancer(&self, id: &Id) -> ApiResult<()> {
        self.stop(id).await;
        Ok(())
    }

    async fn ensure_listener(&self, view: &ListenerView) -> ApiResult<String> {
        if matches!(view.listener.spec.protocol, ListenerProtocol::Udp) {
            return self.ensure_udp(view).await;
        }
        let dial = dial_for(view);
        let id = view.balancer.meta.id.clone();
        let unbound = {
            let slots = self.slots.lock();
            let slot = slots
                .get(&id)
                .ok_or_else(|| ApiError::bad_request("forward balancer is not bound"))?;
            (slot.task.is_none() && slot.listener.is_none()).then_some(slot.bound)
        };
        let fresh = match unbound {
            Some(at) => Some(
                TcpListener::bind(at)
                    .await
                    .map_err(|e| ApiError::unavailable(format!("forward bind {at}: {e}")))?,
            ),
            None => None,
        };
        let mut slots = self.slots.lock();
        let slot = slots
            .get_mut(&id)
            .ok_or_else(|| ApiError::bad_request("forward balancer is not bound"))?;
        *slot.dial.lock() = dial;
        if slot.task.is_none() && slot.listener.is_none() {
            slot.listener = fresh;
        }
        if slot.task.is_none() {
            let listener = slot
                .listener
                .take()
                .ok_or_else(|| ApiError::internal("forward socket is missing"))?;
            let dial = slot.dial.clone();
            let cancel = slot.cancel.clone();
            slot.task = Some(tokio::spawn(async move {
                accept_loop(listener, dial, cancel).await;
            }));
        }
        Ok(slot.bound.to_string())
    }

    async fn delete_listener(&self, balancer: &Id, _listener: &Id) -> ApiResult<()> {
        self.stop(balancer).await;
        Ok(())
    }
}

impl ForwardProvider {
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
        let at = {
            let slots = self.slots.lock();
            let slot = slots
                .get(&id)
                .ok_or_else(|| ApiError::bad_request("forward balancer is not bound"))?;
            if let Some(udp) = &slot.udp {
                udp.set_members(targets);
                return Ok(udp.bound().to_string());
            }
            slot.bound
        };
        // Same address and port the balancer reports.
        let udp = crate::udp::UdpForward::bind(at, targets).await?;
        let bound = udp.bound().to_string();
        let mut slots = self.slots.lock();
        let slot = slots
            .get_mut(&id)
            .ok_or_else(|| ApiError::bad_request("forward balancer is not bound"))?;
        if let Some(existing) = &slot.udp {
            existing.set_members(crate::udp::targets(view));
            drop(udp);
            return Ok(existing.bound().to_string());
        }
        slot.udp = Some(udp);
        if slot.task.is_none() {
            // Free the tcp port held for an ephemeral balancer; a tcp
            // listener added later binds it again.
            slot.listener = None;
        }
        Ok(bound)
    }
}
