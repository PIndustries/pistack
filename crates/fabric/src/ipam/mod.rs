// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! IP and MAC address management.
//!
//! Allocation is deterministic: walk the subnet's pools in order and hand
//! out the first address that is not the network, broadcast or gateway
//! address and is not already held. Callers supply the set of held
//! addresses (scanned from the subnet's ports and the network's floating
//! IPs); this module keeps no state of its own, so it is trivially correct
//! under a single-writer store and easy to test.

mod allocate;
mod broadcast;
mod capacity;
mod format_mac;
mod generate_mac;
mod ip_to_u128;
mod is_reserved;
mod max_allowed_pairs;
mod next_ip;
mod oui;
mod pair_macs;
mod pair_net;
mod parse_mac;
mod pools;
#[cfg(test)]
mod tests;
mod u128_to_ip;
mod validate_fixed;
mod validate_pairs;
mod validate_subnet;
pub use allocate::allocate;
pub use capacity::capacity;
pub use format_mac::format_mac;
pub use generate_mac::generate_mac;
pub use is_reserved::is_reserved;
pub use max_allowed_pairs::MAX_ALLOWED_PAIRS;
pub use next_ip::next_ip;
pub use oui::OUI;
pub use pair_macs::pair_macs;
pub use parse_mac::parse_mac;
pub use pools::pools;
pub use validate_fixed::validate_fixed;
pub use validate_pairs::validate_pairs;
pub use validate_subnet::validate_subnet;
