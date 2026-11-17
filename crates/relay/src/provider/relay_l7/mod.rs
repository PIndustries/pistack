// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! In-process HTTP reverse proxy. One TCP socket per balancer, and a UDP
//! socket when the listener is UDP.
//!
//! `ensure_balancer` binds `127.0.0.1:0` unless `spec.vip_address` is
//! loopback or private. Unspecified, public, multicast, broadcast,
//! link-local, and 100.64/10 addresses are refused. An `Http` listener
//! picks a route by host and path (falling back to the default pool, and
//! answering 404 when neither applies), rewrites the request onto that
//! pool's next member and copies the response back.
//! `Tcp` and `Https` are byte copies. `Terminated` presents the certificate
//! `keep` already stored for that listener and stays unavailable when it
//! has none. This proxy does not generate a certificate. UDP copies
//! datagrams to an up member and is not ready when that list is empty.
//!
//! Members are picked under the pool's algorithm, the same as on the L4
//! forwarder, and the connections this proxy holds count towards
//! `least_conn`.
//!
//! One concern per file: the public [`provider`] and its
//! [`LbProvider`][super::LbProvider] wiring in [`lb`], the per-balancer
//! [`slot`], the bind rules in [`bind`], the route table and member
//! selection in [`table`], the request line and header rewriting in
//! [`http`], and the accept loop and the copy loops over TLS or plain
//! bytes in [`accept`]. Callers see only [`RelayL7Provider`], as before.

mod accept;
mod bind;
mod http;
mod lb;
mod provider;
mod slot;
mod table;

#[cfg(test)]
mod tests;

pub use provider::RelayL7Provider;
