// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::document::DEFAULT_NETWORK;
use crate::types::Pod;
pub(in crate::controllers::pod) fn networks_of(pod: &Pod) -> Vec<String> {
    if pod.spec.networks.is_empty() {
        vec![DEFAULT_NETWORK.into()]
    } else {
        pod.spec.networks.clone()
    }
}
