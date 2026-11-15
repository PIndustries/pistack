// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::probe_target::ProbeTarget;
use pistack_types::Id;

pub(in crate::probe) fn target(id: &Id) -> ProbeTarget {
    ProbeTarget {
        id: id.clone(),
        power_on: true,
        address: Some("127.0.0.1".into()),
    }
}
