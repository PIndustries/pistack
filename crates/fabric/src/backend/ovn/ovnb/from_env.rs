// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::ovn_backend::OvnBackend;
use super::super::pub_consts::{DEFAULT_NB, NB_ENV};

impl OvnBackend {
    /// Endpoint from `PISTACK_OVN_NB`, else [`DEFAULT_NB`].
    pub fn from_env() -> Self {
        let ep = std::env::var(NB_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_NB.to_string());
        Self::new(ep)
    }
}
