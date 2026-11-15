// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::KeyBackend;

impl KeyBackend {
    pub fn name(&self) -> &'static str {
        match self {
            KeyBackend::Software => "software",
            KeyBackend::VaultTransit(_) => "vault",
        }
    }
}
