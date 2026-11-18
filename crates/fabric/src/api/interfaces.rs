// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Interfaces {
    pub subnets: Vec<Id>,
}
