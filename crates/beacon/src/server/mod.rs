// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The authoritative server: a hickory [`RequestHandler`] that answers from
//! the store.
//!
//! Zones are compiled into an in-memory [`Catalog`] on first use and thrown
//! away whenever a `zone` or `rec` event arrives, so an answer is never
//! staler than the last store write. SOA and apex NS records are synthesised
//! from the zone spec and its reconciled serial. UDP and TCP share one port
//! per listen address from [`listen_addrs`].
//!
//! Queries and AXFR (TCP only) are served for primary zones and for secondary
//! zones that have completed an AXFR pull. Updates and IXFR get NOTIMP;
//! NOTIFY is decided by [`crate::notify`]. Secondaries live in a side cache
//! filled by [`crate::xfer`].

mod answer;
mod catalog;
mod compile_zone;
mod default_listen;
mod default_port;
mod handler;
mod host_from_hint;
mod listen_addrs;
mod listen_env;
mod max_cname_depth;
mod parse_listen_list;
mod private_ip_from_hint;
mod proto_type;
mod pulled;
mod resolve_listen_addrs;
mod secondary_cache;
mod serve;
mod synth_owner;
mod tcp_timeout;
#[cfg(test)]
mod tests;
mod zone_data;
pub use answer::Answer;
pub use catalog::Catalog;
pub use default_listen::DEFAULT_LISTEN;
pub use default_port::DEFAULT_PORT;
pub use handler::Handler;
pub use listen_addrs::listen_addrs;
pub use listen_env::LISTEN_ENV;
pub use resolve_listen_addrs::resolve_listen_addrs;
pub use secondary_cache::SecondaryCache;
pub use serve::serve;
pub(crate) use zone_data::ZoneData;
