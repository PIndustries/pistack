// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use hickory_proto::op::{Message, OpCode};
use std::net::SocketAddr;

use super::asked_apex::asked_apex;
use super::is_listed_primary::is_listed_primary;

/// May this NOTIFY be acted on? Only when it names a zone this node holds as
/// secondary and comes from a host that zone lists as a primary.
///
/// The port is deliberately not compared. A NOTIFY arrives from an ephemeral
/// source port — nothing sends one from port 53 — so the address a zone lists
/// identifies a machine, and a second tenant on that machine is the same
/// principal as far as the protocol can tell. What is checked is the zone: a
/// sender trusted for one apex gets nothing for another.
pub(crate) fn notify_allowed(
    client: &Client,
    message: &Message,
    src: SocketAddr,
) -> anyhow::Result<bool> {
    if message.metadata.op_code != OpCode::Notify {
        return Ok(false);
    }
    let Some(apex) = asked_apex(message) else {
        return Ok(false);
    };
    Ok(is_listed_primary(&client.secondary_zones()?, &apex, src))
}
