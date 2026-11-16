// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::slab::MOUNTED_VOLUMES;
use forge::Machine;
use std::collections::BTreeSet;
pub(in crate::controllers::pod) fn mounted_set(m: &Machine) -> BTreeSet<String> {
    m.meta
        .annotations
        .get(MOUNTED_VOLUMES)
        .map(|raw| {
            raw.split(',')
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}
