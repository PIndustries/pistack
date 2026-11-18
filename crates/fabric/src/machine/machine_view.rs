// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

use super::machine_spec_view::MachineSpecView;
use super::machine_status_view::MachineStatusView;

pub(crate) type MachineView = Resource<MachineSpecView, MachineStatusView>;
