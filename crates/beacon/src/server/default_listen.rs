// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Unprivileged loopback fallback when `PISTACK_DNS_LISTEN` is unset and no
/// private cluster address is known. Never the all-interfaces surprise.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:5353";
