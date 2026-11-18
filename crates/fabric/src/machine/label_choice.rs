// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Meta;

use super::auto_fip_label::AUTO_FIP_LABEL;

/// What a machine's or a project's [`AUTO_FIP_LABEL`] asks for.
pub(crate) fn label_choice(meta: &Meta) -> Option<bool> {
    match meta.labels.get(AUTO_FIP_LABEL).map(|v| v.trim()) {
        Some("false") => Some(false),
        Some("true") => Some(true),
        _ => None,
    }
}
