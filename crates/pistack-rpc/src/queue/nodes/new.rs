// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::collections::BTreeMap;

use pistack_types::Resources;

use super::node_info::NodeInfo;

impl NodeInfo {
    pub(crate) fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            capabilities: Vec::new(),
            labels: BTreeMap::new(),
            version: String::new(),
            underlay_ip: None,
            resources_total: Resources::default(),
            resources_free: Resources::default(),
            machines: Vec::new(),
            last_hello: None,
            last_heartbeat: None,
            in_flight: 0,
            queued: 0,
        }
    }
}
