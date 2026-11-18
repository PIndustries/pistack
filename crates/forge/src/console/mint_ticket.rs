// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Instant;
use uuid::Uuid;

use super::ticket::Ticket;
use super::ticket_ttl::TICKET_TTL;
use super::tickets::TICKETS;

/// 32 hex characters. Callers must not write this value to a log.
pub fn mint_ticket(machine_id: &str) -> String {
    let ticket = Uuid::now_v7().simple().to_string();
    let mut map = TICKETS.lock();
    let now = Instant::now();
    map.retain(|_, ticket| ticket.expires > now);
    map.insert(
        ticket.clone(),
        Ticket {
            machine_id: machine_id.to_owned(),
            expires: now + TICKET_TTL,
        },
    );
    ticket
}
