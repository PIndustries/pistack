// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::{Client, Router};
use pistack_types::Id;

pub(crate) fn is_routed(client: &Client, subnet: &Id) -> bool {
    client
        .routers()
        .list(None)
        .ok()
        .into_iter()
        .flatten()
        .any(|r: Router| {
            r.spec.external_network.is_some() && r.spec.interfaces.iter().any(|i| i == subnet)
        })
}
