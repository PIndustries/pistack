// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Absolute `PISTACK_YOUKI_NETNS`, or nothing. Relative paths and `..` are ignored.
use crate::runtime::youki::consts::NETNS_ENV;
use crate::runtime::youki::network::host_netns_from;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn requested_host_netns() -> Option<PathBuf> {
    host_netns_from(std::env::var_os(NETNS_ENV).as_deref())
}
