// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::collections::BTreeMap;

use crate::ca::RESERVED_LABEL_PREFIX;

pub(in crate::client) fn reserved_labels(
    labels: &BTreeMap<String, String>,
) -> Vec<(&String, &String)> {
    labels
        .iter()
        .filter(|(k, _)| k.starts_with(RESERVED_LABEL_PREFIX))
        .collect()
}
