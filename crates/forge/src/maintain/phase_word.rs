// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Phase;

pub(crate) fn phase_word(phase: Phase) -> String {
    format!("{phase:?}").to_lowercase()
}
