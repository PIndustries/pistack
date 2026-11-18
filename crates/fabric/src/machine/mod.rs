// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Read the IPv4 address forge recorded for a machine and, when an admin
//! has asked for it, keep one automatic floating IP pointed at it.
//!
//! Nothing is given out by default. An external network with
//! `spec.auto_floating_ip` is the one pool machines draw from. A machine
//! labelled [`AUTO_FIP_LABEL`]`=false` gets none, and so does every machine
//! of a project labelled that way unless the machine says `true`.
//!
//! fabric does not depend on forge. The view below carries the fields it
//! does not know, so writing a condition puts the rest back unchanged.
//! Floating IPs this pass creates are labelled [`AUTO_FIP_LABEL`]; anything
//! without that label, including a floating IP someone associated by hand,
//! is never deleted. Association does not require a router: host NAT has
//! none, and the mock backend's router check happens later, in reconcile.

mod auto_fip_label;
mod client;
mod fip_condition;
mod first_ipv4;
mod guest_address;
mod guest_ipv4;
mod is_auto;
mod label_choice;
mod labeled_machine;
mod list_machines;
mod listed_machine;
mod machine_label;
mod machine_spec_view;
mod machine_status_view;
mod machine_view;
mod port_covers;
mod reason_of;
mod rest;
#[cfg(test)]
mod tests;
pub(crate) use guest_ipv4::guest_ipv4;
