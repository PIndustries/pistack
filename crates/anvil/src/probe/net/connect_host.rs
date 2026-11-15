// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::due_check::DueCheck;
use crate::probe::book::outcome::Outcome;
use crate::probe::net::host_ok::host_ok;

pub(in crate::probe) fn connect_host<'a>(
    check: &'a DueCheck,
    host: Option<&'a str>,
) -> Result<&'a str, Outcome> {
    if let Some(host) = host.map(str::trim).filter(|h| !h.is_empty()) {
        if host_ok(host) {
            return Ok(host);
        }
        return Err(Outcome::fail("probe host is invalid"));
    }
    match check
        .address
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty())
    {
        Some(addr) if host_ok(addr) => Ok(addr),
        Some(_) => Err(Outcome::fail("sandbox address is invalid")),
        None => Err(Outcome::fail("pod has no address")),
    }
}
