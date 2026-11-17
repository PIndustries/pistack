// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The client: the handle every other module holds, the writes behind it,
//! and the validation each write runs first.
//!
//! [`Client`] is the handle. It hands out the four store collections and
//! the provider a balancer belongs to, and owns the edge queue an edge door
//! needs. Around it: [`balancer_write`], [`listener_write`], [`pool_write`]
//! and [`member_write`] validate and write `spec`, and tell the provider
//! before a delete removes the row; [`lookups`] finds what belongs to what;
//! [`active_members`] is the member set the data planes may dial;
//! [`ensure_tcp_balancer`] is loom's converge path; [`leaves`] is the loom
//! guard the reconciler asks. The four `validate_*` files hold the rules a
//! write enforces, kept apart from the writes so the rule reads on its own.

mod balancer;
mod balancer_write;
mod collections;
mod ensure_tcp_balancer;
mod handle;
mod leaves;
mod listener;
mod listener_write;
mod lookups;
mod member;
mod member_write;
mod pool;
mod pool_write;
mod project;
mod store_err;

pub use handle::Client;
