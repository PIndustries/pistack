// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `StackPhase`: a stack's observed phase.
use serde::{Deserialize, Serialize};

/// Observed phase. `Accepted` until the first apply. Apply moves to
/// `Applying` while it writes rows, then to `Ready` or `Failed`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StackPhase {
    #[default]
    Accepted,
    Applying,
    Ready,
    Failed,
}
