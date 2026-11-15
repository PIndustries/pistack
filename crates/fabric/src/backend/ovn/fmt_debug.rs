// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::ovn_backend::OvnBackend;

impl std::fmt::Debug for OvnBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OvnBackend")
            .field("nb_endpoint", &self.nb_endpoint)
            .finish()
    }
}
