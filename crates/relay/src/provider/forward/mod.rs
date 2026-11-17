// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! In-process L4 forwarder. One TCP socket per balancer, and a UDP socket
//! when the listener is UDP.
//!
//! `ensure_balancer` binds `127.0.0.1:0` unless `spec.vip_address` is
//! loopback or private, in which case it binds that address on an ephemeral
//! port. Unspecified, public, multicast, broadcast, link-local, and
//! 100.64/10 addresses are refused.
//! `provider_ref` is the bound socket, `127.0.0.1:PORT` when the bind
//! address is loopback. The TCP accept loop picks a member under the
//! pool's algorithm and copies both directions. An empty TCP member list,
//! or a member that does not accept, is accepted and closed. UDP copies
//! datagrams to an up member and is not ready when that list is empty. A
//! later `ensure_listener` replaces the member list without rebinding.
//! `delete_listener` and `delete_balancer` stop the task and drop the
//! socket.
//!
//! One concern per file: the dial target in [`dial`], the per-balancer
//! [`slot`], the public [`provider`] and its [`LbProvider`][super::LbProvider]
//! wiring in [`lb`], the bind rules in [`bind`], and the copy loops in
//! [`accept`]. Callers see only [`ForwardProvider`], as before.

mod accept;
mod bind;
mod dial;
mod lb;
mod provider;
mod slot;

#[cfg(test)]
mod tests;

pub use provider::ForwardProvider;
