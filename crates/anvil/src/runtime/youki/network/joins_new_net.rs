// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Stop removed the namespace this pod was created in. The next create has
/// to run again; `youki start` keeps the namespaces from `youki create`.
use crate::runtime::podnet::PodNetwork;

pub(in crate::runtime::youki) fn joins_new_net(net: &PodNetwork) -> bool {
    matches!(net, PodNetwork::Linked(_))
}
