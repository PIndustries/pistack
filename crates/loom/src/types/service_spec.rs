// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::kinds;
use pistack_types::Spec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---- kinds and well-known keys ------------------------------------------------

use super::service_kind::ServiceKind;
use super::service_port::ServicePort;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ServiceSpec {
    #[serde(default)]
    pub selector: BTreeMap<String, String>,
    #[serde(default)]
    pub ports: Vec<ServicePort>,
    #[serde(default)]
    pub kind: ServiceKind,
    /// Ask for a specific cluster IP instead of the next free one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_ip: Option<String>,
}

impl Spec for ServiceSpec {
    const KIND: &'static str = kinds::SERVICE;
}
