// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// `PISTACK_YOUKI_NETNS` when it is set. Otherwise the namespace this runtime created.
use crate::runtime::podnet::PodNetwork;
use crate::runtime::youki::network::requested_host_netns;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn joined_netns(net: &PodNetwork) -> Option<PathBuf> {
    if let Some(path) = requested_host_netns() {
        return Some(path);
    }
    match net {
        PodNetwork::Linked(link) => Some(link.path.clone()),
        PodNetwork::None | PodNetwork::Refused(_) => None,
    }
}
