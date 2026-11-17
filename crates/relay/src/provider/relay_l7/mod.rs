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
//! One file per thing the proxy does. The public [`provider`] and its
//! [`LbProvider`][super::LbProvider] wiring in [`lb`]; the per-balancer
//! [`slot`] and the bind rules in [`bind`]. Member selection, which used to
//! share one file, is now its own set: [`table`] is what requests match
//! against, [`table_for`] builds it, [`choose`] runs a lookup, [`choice`]
//! is its result, [`host_rank`] and [`path_len`] the two comparisons a
//! lookup scores, [`live_backends`] the members a pool offers, [`weighed`]
//! the backends as the picker sees them, [`opened`]/[`closed`] the
//! connection counts `least_conn` reads, and [`held`] one counted
//! connection. The request line and header rewriting in [`http`], and the
//! accept loop in [`accept_loop`], the byte copy in [`splice`], the HTTP
//! proxy in [`proxy_http`], what one listener serves from in [`served`],
//! and the two replies in [`refuse`] and [`not_found`]. Callers see only
//! [`RelayL7Provider`], as before.

mod accept_loop;
mod bind;
mod choice;
mod choose;
mod closed;
mod content_length;
mod find_header_end;
mod header_has;
mod held;
mod host_rank;
mod lb;
mod live_backends;
mod normalize_host;
mod not_found;
mod opened;
mod path_len;
mod provider;
mod proxy_http;
mod read_headers;
mod refuse;
mod rewrite_request;
mod route_key;
mod served;
mod slot;
mod splice;
mod table;
mod table_for;
mod weighed;

#[cfg(test)]
mod tests;

pub use provider::RelayL7Provider;
