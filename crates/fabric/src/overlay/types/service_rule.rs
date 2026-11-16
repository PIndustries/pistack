// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::service_backend::ServiceBackend;
use serde::Deserialize;
use serde::Serialize;

/// A service address answered inside the router namespace. New connections to
/// `address:port` are spread across `backends`. With no backends the router
/// refuses them instead of letting them hang. IPv4 only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceRule {
    pub id: String,
    pub address: String,
    /// `tcp` or `udp`.
    pub protocol: String,
    pub port: u16,
    #[serde(default)]
    pub backends: Vec<ServiceBackend>,
}
