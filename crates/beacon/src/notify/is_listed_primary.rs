// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::ZoneKind;
use std::net::SocketAddr;

use super::dns_port::DNS_PORT;
use super::parse_addr::parse_addr;
use super::same_apex::same_apex;

/// Does this NOTIFY name a secondary zone we hold, from one of its primaries?
pub(crate) fn is_listed_primary(zones: &[crate::types::Zone], apex: &str, src: SocketAddr) -> bool {
    zones.iter().any(|z| {
        if !same_apex(&z.spec.name, apex) {
            return false;
        }
        let ZoneKind::Secondary { primaries } = &z.spec.kind else {
            return false;
        };
        primaries.iter().any(|raw| match parse_addr(raw, DNS_PORT) {
            Ok(p) => p.ip() == src.ip(),
            Err(_) => false,
        })
    })
}
