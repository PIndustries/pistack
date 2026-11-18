// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use serde::{Deserialize, Serialize};

/// A machine that was left where it is, and why.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Left {
    pub id: Id,
    pub name: String,
    pub reason: String,
}
