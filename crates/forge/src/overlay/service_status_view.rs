// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::ServiceEndpointView;
use serde::{Deserialize, Serialize};

/// The allocated cluster address and the ready workload addresses.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ServiceStatusView {
    #[serde(default)]
    pub cluster_ip: Option<String>,
    #[serde(default)]
    pub endpoints: Vec<ServiceEndpointView>,
}
