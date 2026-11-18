// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Host, Machine, NODE_PIN_ANNOTATION};

use super::nonempty_annotation::nonempty_annotation;

pub(crate) fn pinned_hosts(m: &Machine, hosts: &[Host]) -> Option<Vec<Host>> {
    let id = nonempty_annotation(m, NODE_PIN_ANNOTATION)?;
    Some(
        hosts
            .iter()
            .filter(|h| h.meta.id.to_string() == id)
            .cloned()
            .collect(),
    )
}
