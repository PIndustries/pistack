// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;

/// Resource name for an auto-created set: `web.example.cloud.-A` is not a
/// valid name (no `/` is the only rule, but keep it readable).
pub(crate) fn set_name(spec: &RecordSetSpec) -> String {
    format!(
        "{}-{}",
        spec.name.trim_end_matches('.'),
        spec.kind.as_str().to_ascii_lowercase()
    )
}
