// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

use super::percent_decode::percent_decode;

/// `ticket` query value. Missing and empty are the same rejection.
pub fn query_ticket(raw: Option<&str>) -> Result<String, ApiError> {
    let query = raw.unwrap_or("");
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == "ticket" {
            let ticket = percent_decode(value);
            if ticket.is_empty() {
                break;
            }
            return Ok(ticket);
        }
    }
    Err(ApiError::unauthorized("console ticket rejected"))
}
