// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::SocketAddr;

use super::listen_env::LISTEN_ENV;
use super::resolve_listen_addrs::resolve_listen_addrs;

/// Resolve DNS listen addresses.
///
/// * When `PISTACK_DNS_LISTEN` is set (non-empty after trim), parse it as a
///   comma-separated list of `host:port` and honor it exactly.
/// * When unset, prefer a private unicast IPv4 taken from `hint` (the
///   server `--listen` address or `public_url` host — the same private
///   address cluster advertise uses) on port [`DEFAULT_PORT`].
/// * When no private address is known, fall back to [`DEFAULT_LISTEN`].
///   Defaults never use `0.0.0.0` / `::`.
pub fn listen_addrs(hint: Option<&str>) -> anyhow::Result<Vec<SocketAddr>> {
    let env = std::env::var(LISTEN_ENV).ok();
    resolve_listen_addrs(env.as_deref(), hint)
}
