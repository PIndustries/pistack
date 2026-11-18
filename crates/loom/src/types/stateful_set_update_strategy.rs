// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

/// How a StatefulSet rolls a new pod template.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "PascalCase")]
pub enum StatefulSetUpdateStrategy {
    /// Replace pods that do not match the current template, highest ordinal first.
    #[default]
    RollingUpdate,
    /// Leave existing pods alone until they are deleted by hand or scaled away.
    OnDelete,
}
