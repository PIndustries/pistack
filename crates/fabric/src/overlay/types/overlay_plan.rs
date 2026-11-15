// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::segment_plan::SegmentPlan;
use serde::Deserialize;
use serde::Serialize;

/// One host's overlay: a segment per network.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayPlan {
    pub version: u32,
    /// This host's underlay IPv4.
    pub host: String,
    pub networks: Vec<SegmentPlan>,
}
