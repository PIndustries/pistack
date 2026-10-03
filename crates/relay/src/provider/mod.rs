// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The seam between the balancer model and whatever realises it.
//!
//! `relay` owns the resources and decides *what* should exist; an
//! [`LbProvider`] makes it so. Every call is idempotent ("ensure"), takes
//! the fully resolved picture (a listener with its pool and that pool's
//! members already looked up) and returns only what the model cannot know
//! by itself (the VIP that was allocated, a provider-side handle). The
//! reconciler is the only caller.
//!
//! Three implementations: [`MockProvider`] for tests and single-node
//! development, [`OvnProvider`] (L4, a stub documenting the OVN northbound
//! mapping) and [`RelayL7Provider`] (HTTP, a stub documenting the Pingora
//! plan).

use crate::types::{Balancer, Listener, Member, Pool, Provider};
use async_trait::async_trait;
use pistack_types::{ApiResult, Id};
use std::net::IpAddr;
use std::sync::Arc;

pub mod mock;
pub mod ovn;
pub mod relay_l7;

pub use mock::{Call, MockProvider};
pub use ovn::OvnProvider;
pub use relay_l7::RelayL7Provider;

/// What the provider reports once a balancer exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Realized {
    pub vip: IpAddr,
    /// Provider-side handle, stored in `status.provider_ref`.
    pub provider_ref: String,
}

/// A listener with everything the provider needs to program it.
#[derive(Clone, Debug)]
pub struct ListenerView {
    pub balancer: Balancer,
    pub listener: Listener,
    /// Default pool and its members, when the listener has one. Members
    /// that are administratively down or `Offline` are already filtered
    /// out; what remains is what should take traffic.
    pub pool: Option<(Pool, Vec<Member>)>,
}

#[async_trait]
pub trait LbProvider: Send + Sync + 'static {
    /// Human name, for logs and `GET /balancers/`.
    fn name(&self) -> &'static str;

    /// Which `spec.provider` values this implementation serves.
    fn serves(&self, provider: Provider) -> bool;

    /// Create or update the balancer's VIP. `requested` is the spec's
    /// explicit address, if any.
    async fn ensure_balancer(&self, lb: &Balancer) -> ApiResult<Realized>;
    async fn delete_balancer(&self, id: &Id) -> ApiResult<()>;

    /// Program one listener with its pool and members. Returns the
    /// provider-side handle for the listener.
    async fn ensure_listener(&self, view: &ListenerView) -> ApiResult<String>;
    async fn delete_listener(&self, balancer: &Id, listener: &Id) -> ApiResult<()>;
}

/// Route each balancer to the provider that serves its `spec.provider`.
#[derive(Clone, Default)]
pub struct Providers {
    inner: Vec<Arc<dyn LbProvider>>,
}

impl Providers {
    pub fn new(list: Vec<Arc<dyn LbProvider>>) -> Self {
        Self { inner: list }
    }

    pub fn for_balancer(&self, lb: &Balancer) -> ApiResult<Arc<dyn LbProvider>> {
        self.inner
            .iter()
            .find(|p| p.serves(lb.spec.provider))
            .cloned()
            .ok_or_else(|| {
                pistack_types::ApiError::unavailable(format!(
                    "no provider for {:?} balancers",
                    lb.spec.provider
                ))
            })
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.inner.iter().map(|p| p.name()).collect()
    }
}
