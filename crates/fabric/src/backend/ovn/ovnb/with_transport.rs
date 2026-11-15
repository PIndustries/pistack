// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb::Transport;
use super::super::ovn_backend::OvnBackend;
use std::sync::{Arc, OnceLock};

impl OvnBackend {
    pub fn with_transport(nb_endpoint: impl Into<String>, nb: Arc<dyn Transport>) -> Self {
        Self {
            nb_endpoint: nb_endpoint.into(),
            nb,
            attached: OnceLock::new(),
            gateway_chassis: Vec::new(),
        }
    }
}
