// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! EC2-style guest metadata. The process binds loopback only. The host
//! helper DNATs `169.254.169.254:80` on a pistack bridge to this port,
//! and the TCP peer address is the guest's fixed IP.
//!
//! OVN has no such bridge. There the host agent answers the guest from a
//! namespace per network and asks here over `/rpc` ([`agent_routes`]),
//! naming the network the request arrived on and the guest's address. The
//! answer is given only for a guest placed on the asking host.

mod agent_guest;
mod agent_networks;
mod agent_routes;
mod answer;
mod answer_on;
mod default_listen;
mod guest;
mod handle;
mod host_named;
mod host_networks;
mod listen_from_env;
mod lookup;
mod named_secret;
mod parse_listen;
mod respond;
mod secret_names;
mod serve;
#[cfg(test)]
mod tests;
pub use agent_routes::agent_routes;
#[cfg(test)]
pub(crate) use answer::answer;
#[cfg(test)]
pub(crate) use answer_on::answer_on;
#[cfg(test)]
pub(crate) use host_networks::host_networks;
#[cfg(test)]
pub(crate) use parse_listen::parse_listen;
pub use serve::serve;
