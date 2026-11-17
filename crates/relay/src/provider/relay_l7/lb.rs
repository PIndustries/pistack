// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `LbProvider` surface: what a caller asks the proxy to bind, serve,
//! and release.

use super::accept_loop::accept_loop;
use super::bind::bind_target;
use super::provider::RelayL7Provider;
use super::served::Served;
use super::slot::{Serve, Slot};
use super::table_for::table_for;
use crate::provider::{LbProvider, ListenerView, Realized};
use crate::types::{Balancer, ListenerProtocol, Provider};
use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult, Id};
use std::net::SocketAddr;
use tokio::net::TcpListener;

fn realized(bound: SocketAddr) -> Realized {
    Realized {
        vip: bound.ip(),
        provider_ref: bound.to_string(),
    }
}

#[async_trait]
impl LbProvider for RelayL7Provider {
    fn name(&self) -> &'static str {
        "http"
    }

    fn serves(&self, provider: Provider) -> bool {
        provider == Provider::Relay
    }

    async fn ensure_balancer(&self, lb: &Balancer) -> ApiResult<Realized> {
        if let Some(bound) = self.slots.lock().get(&lb.meta.id).map(|s| s.bound) {
            return Ok(realized(bound));
        }
        let target = bind_target(lb)?;
        let listener = TcpListener::bind(target)
            .await
            .map_err(|e| ApiError::unavailable(format!("http bind {target}: {e}")))?;
        let bound = listener
            .local_addr()
            .map_err(|e| ApiError::unavailable(format!("http local address: {e}")))?;
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
        let serve = match &view.listener.spec.protocol {
            ListenerProtocol::Udp => unreachable!("udp returned above"),
            ListenerProtocol::Terminated { .. } => {
                let material = view
                    .tls
                    .as_ref()
                    .ok_or_else(|| ApiError::unavailable("no certificate for this listener"))?;
                Serve::Tls(crate::tls::server_config(material)?)
            }
            ListenerProtocol::Http => Serve::Http,
            ListenerProtocol::Tcp | ListenerProtocol::Https => Serve::Stream,
        };
        let table = table_for(view);
        let algo = view
            .pool
            .as_ref()
            .map(|(pool, _)| pool.spec.algorithm)
            .unwrap_or_default();
        let limit = view.listener.spec.connection_limit as usize;
        let id = view.balancer.meta.id.clone();
        let mut slots = self.slots.lock();
        let slot = slots
            .get_mut(&id)
            .ok_or_else(|| ApiError::bad_request("http balancer is not bound"))?;
        slot.picker.set_algorithm(algo);
        *slot.members.lock() = table;
        slot.limit
            .store(limit, std::sync::atomic::Ordering::Relaxed);
        *slot.mode.lock() = serve;
        if slot.task.is_none() {
            let listener = slot
                .listener
                .take()
                .ok_or_else(|| ApiError::internal("http socket is missing"))?;
            let served = Served::for_view(slot);
            let cancel = slot.cancel.clone();
            slot.task = Some(tokio::spawn(async move {
                accept_loop(listener, served, cancel).await;
            }));
        }
        Ok(slot.bound.to_string())
    }

    async fn delete_listener(&self, balancer: &Id, _listener: &Id) -> ApiResult<()> {
        self.stop(balancer).await;
        Ok(())
    }
}
