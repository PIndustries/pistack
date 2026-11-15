// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::outcome::Outcome;

impl Outcome {
    pub(crate) fn ok() -> Self {
        Self {
            success: true,
            unsupported: false,
            message: String::new(),
        }
    }
}
