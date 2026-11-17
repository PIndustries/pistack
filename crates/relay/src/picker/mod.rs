// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Member selection. Both data planes ask here so a pool balances the
//! same way over TCP and over HTTP.
//!
//! The unit is a [`Backend`]: the address to dial, its weight, and how
//! many connections are open on it. Everything is keyed by address, so a
//! member list in a new order keeps its place rather than reshuffling the
//! traffic.
//!
//! * `round_robin` visits the backends in proportion to their weight, so
//!   a backend of weight 2 takes twice the share.
//! * `least_conn` takes the backend with the fewest open connections,
//!   breaking a tie by weight and then by order.
//! * `source_ip` maps a client address to one backend and keeps it there
//!   while the set of backends is unchanged.
//!
//! A backend with weight 0 takes no new connections, so it never appears
//! as a choice. Members that are not healthy never reach a picker at all:
//! the caller hands over only what may take traffic.
//!
//! One file per thing: the [`Backend`] in [`backend`], the members that
//! may become one in [`eligible`], the algorithm names a pool spec may
//! carry in [`algorithm`], the [`Picker`] and its dispatch in [`select`],
//! the rotation behind `round_robin` in [`round_robin`], the counts read
//! by `least_conn` in [`least_conn`], and the ring and pins behind
//! `source_ip` in [`source_ip`], over the address hash in [`hash_ip`]. Callers see [`Backend`] and [`Picker`], as before.

mod algorithm;
mod backend;
mod eligible;
mod hash_ip;
mod least_conn;
mod round_robin;
mod select;
mod source_ip;

#[cfg(test)]
mod tests;

pub use algorithm::wire;
pub use backend::Backend;
pub use eligible::{dialable, eligible};
pub use select::Picker;
