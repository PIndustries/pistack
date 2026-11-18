// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::FloatingIp;
use pistack_types::{kind, Id};

use super::machine_label::MACHINE_LABEL;

pub(crate) fn labeled_machine(fip: &FloatingIp) -> Option<Id> {
    fip.meta
        .labels
        .get(MACHINE_LABEL)
        .and_then(|s| Id::parse(kind::MACHINE, s))
}
