// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use serde::{Deserialize, Serialize};

/// A machine that is on its way to another host.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Moving {
    pub id: Id,
    pub name: String,
    /// Destination host id.
    pub to: Id,
    /// Destination host name. Empty when the host record is gone.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub to_name: String,
}
