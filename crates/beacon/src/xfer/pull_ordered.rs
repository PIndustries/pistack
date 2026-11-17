// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::server::ZoneData;
use std::net::SocketAddr;

use super::parse_primary::parse_primary;
use super::pull_axfr::pull_axfr;

pub(crate) async fn pull_ordered(
    first: &[String],
    rest: &[String],
    apex: &str,
) -> anyhow::Result<(u32, ZoneData, String)> {
    let mut last_err = anyhow::anyhow!("no primaries configured");
    let mut tried: Vec<SocketAddr> = Vec::new();
    for raw in first.iter().chain(rest) {
        let addr = match parse_primary(raw) {
            Ok(a) => a,
            Err(e) => {
                last_err = e;
                continue;
            }
        };
        if tried.contains(&addr) {
            continue;
        }
        tried.push(addr);
        match pull_axfr(addr, apex).await {
            Ok((serial, data)) => return Ok((serial, data, raw.clone())),
            Err(e) => {
                tracing::warn!(primary = %raw, apex, error = %e, "beacon: secondary AXFR failed");
                last_err = e;
            }
        }
    }
    Err(last_err)
}
