// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

/// `Ok` — latest sample does not cross. `Firing` — it does.
/// `Pending` — no sample for this metric yet.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum AlarmPhase {
    #[default]
    Pending,
    Ok,
    Firing,
}
