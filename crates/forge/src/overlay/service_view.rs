// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::ServicePortView;
use pistack_types::Spec;
use serde::{Deserialize, Serialize};

/// The parts of a workload service record the project router needs. The
/// workload controller owns the record; this view only reads it and must
/// never be written back, since it drops every other field.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ServiceView {
    #[serde(default)]
    pub ports: Vec<ServicePortView>,
}

impl Spec for ServiceView {
    const KIND: &'static str = "svc";
}
