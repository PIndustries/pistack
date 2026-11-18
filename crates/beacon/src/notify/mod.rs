// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Zone change notification (RFC 1996), both directions.
//!
//! Primary side: when a zone's serial moves, every address listed as a
//! primary of a secondary zone for that apex is told over UDP with a
//! `NOTIFY` for `<apex> SOA`, carrying the new serial as the answer hint.
//! Each send is repeated with backoff up to [`ATTEMPTS`] times until the peer
//! answers; the answer is the acknowledgement, and `NOTIMP` counts as one — a
//! peer that will not take notifies will never take them. A zone whose
//! notifies go unanswered keeps its old mark, so a later pass tries again.
//!
//! Secondary side: a NOTIFY is accepted only from an address configured as a
//! primary of a secondary zone this node holds. The zone is then pulled at
//! once instead of at the next refresh. Anything else — an unknown sender, a
//! zone we do not hold, a NOTIFY at a primary — is answered `REFUSED` and
//! pulls nothing.
//!
//! Neither side blocks a caller. Nothing here is on the request path:
//! [`run_notifies`] watches the event bus, gives a burst one window to settle,
//! then acts on what the serials actually say. The notify loop runs wherever the
//! reconciler runs — the leader — and reads secondaries from the same
//! replicated store, so one node notifies for the whole cloud. The receiving
//! side runs on every node that serves DNS, because a NOTIFY can arrive at
//! any of them.

mod ack_timeout;
mod answer_notify;
mod asked_apex;
mod attempts;
mod backoff;
mod coalense;
mod dns_port;
mod exchange;
mod failed;
mod is_listed_primary;
mod next_id;
mod notified_key;
mod notifier;
mod notify_allowed;
mod notify_answer_header;
mod notify_bytes;
mod notify_message;
mod notify_moved;
mod notify_one;
mod notify_peers;
mod notify_zones;
mod parse_addr;
mod peers_for;
mod record;
mod run_notifies;
mod run_pulls;
mod same_apex;
mod send_notify;
#[cfg(test)]
mod tests;
pub use ack_timeout::ACK_TIMEOUT;
pub(crate) use answer_notify::answer_notify;
pub(crate) use asked_apex::asked_apex;
pub use attempts::ATTEMPTS;
pub use backoff::BACKOFF;
pub use coalense::COALENSE;
pub use dns_port::DNS_PORT;
pub use notifier::Notifier;
pub(crate) use notify_allowed::notify_allowed;
pub use notify_message::notify_message;
pub use notify_one::notify_one;
pub use notify_peers::notify_peers;
pub use parse_addr::parse_addr;
pub use peers_for::peers_for;
pub(crate) use run_notifies::run_notifies;
pub(crate) use run_pulls::run_pulls;
pub(crate) use same_apex::same_apex;
