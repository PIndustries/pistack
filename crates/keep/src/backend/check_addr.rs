// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::IpAddr;

use super::backend_err::backend_err;
use super::consts::ENV_VAULT_ADDR;
use crate::error::Result;

/// `https://` anywhere; `http://` only to loopback, so a token never
/// crosses the network in the clear.
pub(in crate::backend) fn check_addr(addr: &str) -> Result<()> {
    let url = reqwest::Url::parse(addr)
        .map_err(|e| backend_err(format!("{ENV_VAULT_ADDR}={addr:?} is not a URL: {e}")))?;
    let host = url.host_str().unwrap_or_default();
    let loopback = host == "localhost"
        || host
            .trim_matches(|c| c == '[' || c == ']')
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    match url.scheme() {
        "https" => Ok(()),
        "http" if loopback => Ok(()),
        "http" => Err(backend_err(format!(
            "{ENV_VAULT_ADDR}={addr:?}: plain http is only allowed to a loopback address; use https"
        ))),
        s => Err(backend_err(format!(
            "{ENV_VAULT_ADDR}={addr:?}: scheme {s:?} is not http or https"
        ))),
    }
}
