// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::slab::SlabCondition;
use pistack_types::{Id, Phase};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(in crate::controllers::pod) struct SlabVolStatus {
    #[serde(default)]
    pub(in crate::controllers::pod) phase: Phase,
    #[serde(default)]
    pub(in crate::controllers::pod) attached_to: Option<Id>,
    #[serde(default)]
    pub(in crate::controllers::pod) device: Option<String>,
    #[serde(default)]
    pub(in crate::controllers::pod) backend_ref: Option<String>,
    /// Host path, when storage records it separately from `backend_ref`.
    #[serde(default)]
    pub(in crate::controllers::pod) host_path: Option<String>,
    /// Host id the volume is attached on, when storage records one.
    #[serde(default)]
    pub(in crate::controllers::pod) host: Option<String>,
    #[serde(default)]
    pub(in crate::controllers::pod) conditions: Vec<SlabCondition>,
}
