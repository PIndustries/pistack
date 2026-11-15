// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::{Port, PortSpec};
use pistack_types::{kind, Id, Meta};

pub(in crate::overlay) fn sg_port(ips: &[&str], group: &Id) -> Port {
    Port {
        meta: Meta::new(kind::PORT, "p", None),
        spec: PortSpec {
            fixed_ips: ips.iter().map(|ip| ip.parse().unwrap()).collect(),
            security_groups: vec![group.clone()],
            ..PortSpec::default()
        },
        status: Default::default(),
    }
}
