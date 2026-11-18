// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::kinds;
use pistack_types::Spec;
use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::pod_template::PodTemplate;
use super::selector::Selector;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonSetSpec {
    /// Defaults to the template labels when empty.
    #[serde(default)]
    pub selector: Selector,
    pub template: PodTemplate,
}

impl Spec for DaemonSetSpec {
    const KIND: &'static str = kinds::DAEMON_SET;
}
