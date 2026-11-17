// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Addresses a listener may bind.
//!
//! Loopback, RFC 1918, or IPv6 unique local. Unspecified, public, multicast,
//! broadcast, link-local, and 100.64/10 are refused.
//!
//! [`allowed`] is the question a forwarder or the HTTP proxy asks; it sends
//! v4 to [`v4_allowed`] and v6 to [`v6_allowed`], which lean on [`is_rfc1918`]
//! and [`is_shared`]. Callers see [`allowed`] here, as before.

mod allowed;
mod is_rfc1918;
mod is_shared;
mod v4_allowed;
mod v6_allowed;

pub use allowed::allowed;

#[cfg(test)]
mod tests;
