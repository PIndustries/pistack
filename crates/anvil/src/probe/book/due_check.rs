// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::spec::probe_action::ProbeAction;
use pistack_types::Id;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct DueCheck {
    pub machine: Id,
    pub generation: u64,
    pub container: String,
    pub kind: String,
    pub action: ProbeAction,
    pub timeout: Duration,
    pub address: Option<String>,
}
