// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;
use std::time::Instant;

use super::tickets::TICKETS;

/// Drop the ticket. A second upgrade with the same value is rejected.
pub fn take_ticket(ticket: &str, machine_id: &str) -> Result<(), ApiError> {
    let mut map = TICKETS.lock();
    match map.remove(ticket) {
        Some(found) if found.expires > Instant::now() && found.machine_id == machine_id => Ok(()),
        Some(found) => {
            map.insert(ticket.to_owned(), found);
            Err(ApiError::unauthorized("console ticket rejected"))
        }
        None => Err(ApiError::unauthorized("console ticket rejected")),
    }
}
