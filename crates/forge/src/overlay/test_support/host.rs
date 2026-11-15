// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{new_host, Host};
use pistack_types::{Phase, Resources};
use std::collections::BTreeMap;

pub(in crate::overlay) fn host(name: &str, underlay: &str) -> Host {
    let mut host = new_host(
        name,
        name,
        Resources {
            vcpus: 4,
            memory_mib: 8192,
            disk_gib: 100,
            gpus: 0,
        },
        vec!["kvm".into(), "libvirt".into()],
        BTreeMap::new(),
    );
    host.spec.underlay_ip = Some(underlay.into());
    host.status.phase = Phase::Ready;
    host
}
