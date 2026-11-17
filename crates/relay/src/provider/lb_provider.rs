// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The seam a provider implements: ensure and delete a balancer and its
//! listeners, idempotently.

use super::listener_view::ListenerView;
use super::realized::Realized;
use crate::types::Balancer;
use crate::types::Provider;
use async_trait::async_trait;
use pistack_types::{ApiResult, Id};

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
