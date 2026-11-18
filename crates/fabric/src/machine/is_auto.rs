// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::FloatingIp;

use super::auto_fip_label::AUTO_FIP_LABEL;

pub(crate) fn is_auto(fip: &FloatingIp) -> bool {
    fip.meta.labels.get(AUTO_FIP_LABEL).map(String::as_str) == Some("true")
}
