// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;
use std::time::Instant;

use super::tickets::TICKETS;

/// True when the ticket is live and was minted for `machine_id`.
/// Does not consume it. A failed `virsh` can try the same ticket again.
pub fn check_ticket(ticket: &str, machine_id: &str) -> Result<(), ApiError> {
    let map = TICKETS.lock();
    match map.get(ticket) {
        Some(found) if found.expires > Instant::now() && found.machine_id == machine_id => Ok(()),
        _ => Err(ApiError::unauthorized("console ticket rejected")),
    }
}
