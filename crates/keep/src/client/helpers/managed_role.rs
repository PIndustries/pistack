// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::collections::BTreeMap;

use crate::ca::ROLE_LABEL;

/// The keep role (`keep.pistack.io/role`) of a secret keep manages itself.
pub(crate) fn managed_role(labels: &BTreeMap<String, String>) -> Option<&str> {
    labels.get(ROLE_LABEL).map(String::as_str)
}
