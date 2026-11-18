// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Private-subnet discovery. WireGuard does not carry a broadcast, so a
//! new node unicasts to each address in its own prefix. The prefix must be
//! /24 or longer. `0.0.0.0` and public ranges are refused.

mod announce;
mod check_advertise;
mod collect_announcements;
mod discover_error;
mod discover_port;
mod port_unreachable;
mod scan_targets;
mod send_datagram;
mod sweep;
#[cfg(test)]
mod tests;
pub use announce::Announce;
pub use check_advertise::check_advertise;
pub use collect_announcements::collect_announcements;
pub use discover_error::DiscoverError;
pub use discover_port::DISCOVER_PORT;
pub use scan_targets::scan_targets;
pub use sweep::sweep;
