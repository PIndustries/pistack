// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! relay's own L7 provider, built on Pingora. Today a stub that returns
//! `not_implemented`; this file documents the plan so the dependency can be
//! added deliberately rather than pulled in by the model.
//!
//! # Plan
//!
//! Pingora (Cloudflare's Rust proxy framework) runs in-process in every
//! `pistack agent` that carries the `relay` role, as a `pingora::server::
//! Server` owned by `anvil`. The control plane does not proxy traffic; it
//! ships configuration to those agents over `pistack-rpc`.
//!
//! * **Balancer** -> one VIP, realised exactly as the OVN provider does
//!   (a fabric port, optionally a floating IP). The VIP is answered by the
//!   agents in an OVN `Load_Balancer` row of protocol `tcp` whose members
//!   are the relay agents' own addresses, so L4 distribution across proxy
//!   instances is OVN's job and L7 is Pingora's. `provider_ref` is the id
//!   of that row.
//! * **Listener** `Http` -> a `pingora::listeners::Listeners` TCP endpoint
//!   on `<vip>:<port>` with an `HttpProxy` service. `Terminated{cert}` ->
//!   the same with `TlsSettings` whose cert and key come from `keep`
//!   (`keep::Client::certificate(cert)`), rotated on `cert` events.
//!   `connection_limit` -> the service's `max_connections`.
//! * **Pool** -> a `pingora_load_balancing::LoadBalancer<S>` where
//!   `S` is `RoundRobin`, `Weighted<fnv>` for `SourceIp` (hash on client
//!   address) and a custom least-connections selector for `LeastConn`.
//!   `SessionPersistence::HttpCookie`/`AppCookie` -> a `ProxyHttp::
//!   upstream_peer` that reads the cookie, maps it to a backend and sets it
//!   on the way out.
//! * **Member** -> a `Backend` in the pool with `weight`; `backup` members
//!   are a second pool consulted when the primary is empty. Pingora's own
//!   `health_check::TcpHealthCheck`/`HttpHealthCheck` is turned off and the
//!   backend set is driven from `Member.status.operating_status` produced by
//!   `health.rs`, so both providers see one truth.
//! * **Health monitor** `Http`/`Https` -> `health.rs` probes `path` and
//!   compares the status code with `expected_codes`.
//!
//! Configuration flows as one `RelayConfig { listeners: Vec<ListenerConfig> }`
//! document per agent, versioned by the max `observed_revision` it covers;
//! agents apply it with Pingora's graceful upgrade (new listeners start,
//! old ones drain). Access logs and per-upstream metrics go to `gauge`.
//!
//! Nothing here pulls in `pingora` yet; it arrives with the agent side.

use super::{LbProvider, ListenerView, Realized};
use crate::types::{Balancer, Provider};
use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult, Id};

#[derive(Default)]
pub struct RelayL7Provider;

impl RelayL7Provider {
    fn todo(what: &str) -> ApiError {
        ApiError::not_implemented(format!(
            "relay l7 (pingora) provider: {what} not implemented yet"
        ))
    }
}

#[async_trait]
impl LbProvider for RelayL7Provider {
    fn name(&self) -> &'static str {
        "relay-l7"
    }

    fn serves(&self, provider: Provider) -> bool {
        provider == Provider::Relay
    }

    async fn ensure_balancer(&self, _lb: &Balancer) -> ApiResult<Realized> {
        Err(Self::todo("ensure_balancer"))
    }

    async fn delete_balancer(&self, _id: &Id) -> ApiResult<()> {
        Err(Self::todo("delete_balancer"))
    }

    async fn ensure_listener(&self, _view: &ListenerView) -> ApiResult<String> {
        Err(Self::todo("ensure_listener"))
    }

    async fn delete_listener(&self, _balancer: &Id, _listener: &Id) -> ApiResult<()> {
        Err(Self::todo("delete_listener"))
    }
}
