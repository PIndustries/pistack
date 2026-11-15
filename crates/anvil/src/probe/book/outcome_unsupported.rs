// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::outcome::Outcome;

impl Outcome {
    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self {
            success: false,
            unsupported: true,
            message: message.into(),
        }
    }
}
