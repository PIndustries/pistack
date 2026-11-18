// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::SocketAddr;
use std::str::FromStr;

use super::listen_env::LISTEN_ENV;

/// Split a comma-separated `host:port` list. Empty segments are skipped.
pub(crate) fn parse_listen_list(raw: &str) -> anyhow::Result<Vec<SocketAddr>> {
    let mut out = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let addr = SocketAddr::from_str(part)
            .map_err(|e| anyhow::anyhow!("{LISTEN_ENV} entry {part:?}: {e}"))?;
        out.push(addr);
    }
    if out.is_empty() {
        anyhow::bail!("{LISTEN_ENV} is empty after parsing");
    }
    Ok(out)
}
