// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Meta;
use serde::Deserialize;

use super::machine_status_view::MachineStatusView;

#[derive(Deserialize)]
pub(crate) struct MachineView {
    #[serde(flatten)]
    pub(crate) meta: Meta,
    #[serde(default)]
    pub(crate) status: MachineStatusView,
}
