// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::kinds;
use pistack_types::Spec;
use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::one::one;
use super::scale_target_kind::ScaleTargetKind;

/// Scale a Deployment or StatefulSet from a gauge metric.
///
/// `desired = ceil(current * metric / target_value)`, clamped to
/// `[min_replicas, max_replicas]`. When `current` is zero the ratio uses
/// one replica as the base so a high metric can still scale out.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AutoscalerSpec {
    pub target_kind: ScaleTargetKind,
    pub target_name: String,
    pub metric: String,
    pub target_value: f64,
    #[serde(default = "one")]
    pub min_replicas: u32,
    pub max_replicas: u32,
}

impl Spec for AutoscalerSpec {
    const KIND: &'static str = kinds::AUTOSCALER;
}
