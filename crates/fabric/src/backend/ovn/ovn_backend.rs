// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::ovsdb::Transport;
use super::attached::Attached;
use std::sync::{Arc, OnceLock};

/// Writes OVN's northbound database.
pub struct OvnBackend {
    /// e.g. `unix:/var/run/ovn/ovnnb_db.sock` or `tcp:10.0.0.11:6641`.
    pub nb_endpoint: String,
    pub(in crate::backend::ovn) nb: Arc<dyn Transport>,
    pub(in crate::backend::ovn) attached: OnceLock<Attached>,
    /// Chassis for router gateway ports, highest priority first.
    pub(in crate::backend::ovn) gateway_chassis: Vec<String>,
}
