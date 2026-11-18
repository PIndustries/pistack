// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::kinds;
use pistack_types::Spec;
use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::ingress_backend::IngressBackend;
use super::ingress_rule::IngressRule;
use super::ingress_tls::IngressTls;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct IngressSpec {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ingress_class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_backend: Option<IngressBackend>,
    #[serde(default)]
    pub rules: Vec<IngressRule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tls: Vec<IngressTls>,
}

impl Spec for IngressSpec {
    const KIND: &'static str = kinds::INGRESS;
}
