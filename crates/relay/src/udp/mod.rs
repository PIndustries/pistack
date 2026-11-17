// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! UDP datagram copy. One socket per listener, on the balancer's bind
//! address. A client stays on one up member until that member leaves the
//! pool. An empty member list does not become a running forwarder.
//!
//! One file per thing: [`UdpForward`] is the bound forwarder and its
//! [`bind`](UdpForward::bind), [`run`] the loop that reads the listener's
//! datagrams and routes them, [`pick`] the round-robin choice for a client
//! with no session yet, [`open_session`] the per-client socket that carries
//! a client's datagrams to one member and its replies back, [`session`] the
//! record of that pairing, [`targets`] the members a listener may send to
//! and [`is_up`] the member check it runs. Callers see [`UdpForward`] and
//! [`targets`] here, as before.

mod is_up;
mod open_session;
mod pick;
mod run;
mod session;
mod targets;
mod udp_forward;

pub use targets::targets;
pub use udp_forward::UdpForward;
