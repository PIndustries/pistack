// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Environment variable naming the listen address(es).
///
/// A single `host:port`, or a comma-separated list for multi-bind
/// (for example `10.66.66.1:5353,127.0.0.1:5353`). When set, the value is
/// honored exactly — including `0.0.0.0:53` for production (needs
/// `CAP_NET_BIND_SERVICE`). When unset, see [`listen_addrs`].
pub const LISTEN_ENV: &str = "PISTACK_DNS_LISTEN";
