// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `relay`: load balancers. L4 is an in-process TCP forwarder. HTTP is an
//! in-process reverse proxy. Both sit behind [`provider::LbProvider`].
//!
//! Layout:
//! * [`types`] — `Balancer`, `Listener`, `Pool`, `Member`.
//! * [`Client`] — validation and writes; the library API other modules use
//!   ([`Client::ensure_tcp_balancer`] for `loom` services).
//! * [`provider`] — the `LbProvider` trait, `MockProvider`, `ForwardProvider`,
//!   and the HTTP reverse proxy.
//! * [`door`] — publish a listener. Cloudflared and tailscale run when the
//!   binary is present. WireGuard creates only a `pw` interface and is
//!   unavailable when `wg` is missing. Edge queues a publish for a host
//!   whose driver is `edge` and starts no process.
//! * `reconcile` — turns spec into provider state and writes status.
//! * [`health`] — active TCP/HTTP checks driving `Member.operating_status`.
//! * [`picker`] — member selection under the pool's algorithm.
//! * `api` — HTTP, mounted at `/balancers`.
//! * [`RelayBackend`] and [`Relay`] — the module a server mounts, and the
//!   backend choice that picks its providers.

pub mod api;
pub mod bind;
pub mod client;
pub mod door;
pub mod health;
pub mod picker;
pub mod provider;
mod reconcile;
pub(crate) mod spawn_retry;
mod tls;
pub mod types;
mod udp;

pub use client::Client;
pub use door::{DoorKind, DoorPlan};
pub use picker::{Backend, Picker};
pub use provider::{
    Call, ForwardProvider, LbProvider, MockProvider, OvnProvider, Providers, RelayL7Provider,
    RouteView,
};
pub use types::*;

mod backend;
mod relay;

pub use backend::RelayBackend;
pub use relay::Relay;

#[cfg(test)]
#[path = "backend_tests.rs"]
mod backend_tests;
