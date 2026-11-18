// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;
use pistack_types::{kind, Meta, Phase, Resource, Resources};
use std::collections::BTreeMap;

use super::host::Host;
use super::host_spec::HostSpec;
use super::host_status::HostStatus;

/// Build a host resource. `labels` land in `meta.labels` so the scheduler's
/// `host_selector` can match them.
pub fn new_host(
    name: impl Into<String>,
    node_name: impl Into<String>,
    capacity: Resources,
    drivers: Vec<String>,
    labels: BTreeMap<String, String>,
) -> Host {
    let mut meta = Meta::new(kind::HOST, name, None);
    meta.labels = labels;
    Host(Resource {
        meta,
        spec: HostSpec {
            node_name: node_name.into(),
            capacity,
            drivers,
            ..Default::default()
        },
        status: HostStatus {
            free: capacity,
            last_seen: Some(Utc::now()),
            phase: Phase::Ready,
            ..Default::default()
        },
    })
}
