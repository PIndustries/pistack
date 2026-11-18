// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Browser console for a machine.
//!
//! The portal asks for a ticket with the caller's bearer token. The
//! WebSocket that follows carries only that ticket: a browser cannot set
//! `Authorization` on a WebSocket. The ticket lives in this process for five
//! minutes, is bound to the machine id, and is consumed when the upgrade
//! starts. The TCP connection is always `127.0.0.1` and the display number
//! comes from `virsh vncdisplay`. Nothing here publishes the VNC port.

mod check_ticket;
mod display_port;
mod domain_name;
mod is_loopback;
mod mint_ticket;
mod percent_decode;
mod query_ticket;
mod splice;
mod split_display;
mod take_ticket;
#[cfg(test)]
mod tests;
mod ticket;
mod ticket_ttl;
mod tickets;
mod virsh_timeout;
mod vnc_port;
pub use check_ticket::check_ticket;
pub use display_port::display_port;
pub use domain_name::domain_name;
pub use mint_ticket::mint_ticket;
pub use query_ticket::query_ticket;
pub use splice::splice;
pub use take_ticket::take_ticket;
#[cfg(test)]
pub(crate) use ticket::Ticket;
#[cfg(test)]
pub(crate) use ticket_ttl::TICKET_TTL;
#[cfg(test)]
pub(crate) use tickets::TICKETS;
pub use vnc_port::vnc_port;
