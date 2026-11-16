// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use serde::{Deserialize, Serialize};

use super::chassis_view::ChassisView;
use pistack_types::Phase;

/// Host state the gateway choice reads. See [`HostView`].
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HostStatusView {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default)]
    pub ovn: Option<ChassisView>,
}
