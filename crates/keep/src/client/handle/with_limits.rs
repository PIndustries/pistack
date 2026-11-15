// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::quota::KeepConfig;

impl Client {
    /// Replace the count limits (normally `[modules.keep]`).
    pub fn with_limits(mut self, limits: KeepConfig) -> Self {
        self.limits = limits;
        self
    }
}
