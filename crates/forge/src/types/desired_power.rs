// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

/// Desired power state. `pi machine stop` flips this to `Off`; the
/// reconciler makes it true and records the observed state in `status`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum DesiredPower {
    #[default]
    On,
    Off,
}
